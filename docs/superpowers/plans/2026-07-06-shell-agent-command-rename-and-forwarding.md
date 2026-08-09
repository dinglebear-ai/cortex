# Shell/Agent CLI Rename + Command-Log Forwarding + Stale-Timer Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the redundant, hyphen-heavy `cortex ingest agent-command ingest-spool` / `cortex ingest agent-command wrap` CLI grammar with a `cortex ingest shell user ...` / `cortex ingest shell agent ...` structure, add a nested `setup shell completions` install step, let the agent-command drain path forward records to a remote production Cortex instead of only writing a local DB, and give `cortex doctor` a lightweight way to flag systemd user units still invoking stale agent-command CLI grammar.

**Architecture:** The CLI already nests one level under `ingest` (e.g. `ingest shell index`, `ingest file-tail add`). This plan adds a second nesting level under `shell` (`user` vs `agent`) so the AI-agent-issued-command capture domain no longer needs its own top-level hyphenated name, and no longer collides with the unrelated `heartbeat_agent`/fleet-deploy-agent naming already used elsewhere in this codebase. Forwarding reuses the existing `--server`/`--token` global-flag plumbing and the exact POST-then-truncate-on-success pattern the heartbeat agent already uses (`src/heartbeat_agent.rs` → `src/heartbeat.rs`), giving the new `/v1/agent-commands` endpoint a direct precedent to mirror. Stale-timer detection reuses the existing `src/setup/systemd.rs` helpers rather than building new systemd-management machinery — it only reads and reports, plus an opt-in `--fix` to disable a stale unit.

**Tech Stack:** Rust 2024 edition, axum (HTTP), reqwest (HTTP client), rusqlite, systemd user units (via `systemctl --user`), existing `cortex` CLI/service-layer conventions.

## Global Constraints

- No new hyphenated CLI subcommand words anywhere in this plan — the whole point is eliminating `agent-command` / `ingest-spool`. New words introduced: `user`, `agent`, `index`, `completions` (all single words, no hyphens). This applies to both `ingest` (`ingest shell user ...` / `ingest shell agent ...`) and `setup` (`setup shell agent ...` / `setup shell completions ...`) — `setup` gets a new `shell` grouping exactly like `ingest` did, rather than keeping its older flat hyphenated siblings (`heartbeat-agent`, `debug-wrapper`, etc.) as precedent. Those older siblings are untouched by this plan; only the agent-command/completions surfaces move under `setup shell`.
- Data-model identity (`SourceKind::AgentCommand` → wire string `"agent-command"`, DB `source_type`/`source_kind` values, MCP schema enum entries) is **out of scope** and must NOT change — only CLI-facing grammar changes. Existing rows and cross-host consistency depend on that string staying stable.
- Backward compatibility: the already-installed wrapper script and any manually-created systemd timers on live hosts (e.g. `devhost`) still invoke the pre-rename grammar until `cortex setup shell agent install` is rerun. The CLI parser must keep accepting the immediately-prior grammar (`cortex ingest agent-command {ingest-spool|wrap}`) as a deprecated alias so an unattended host is never bricked by this rename the same way bead `syslog-mcp-4n4a6` happened.
- `cargo xtask bump-version patch|minor|major` and a `CHANGELOG.md` entry are required before this lands on `main`, per this repo's CLAUDE.md. This plan does not itself dictate which bump size — pick per the final commit's conventional-commit prefix when you open the PR.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` and `cargo fmt --check` must pass (repo's `lefthook.yml` pre-push gate enforces this already).

---

## File Structure

| File | Responsibility |
|---|---|
| `src/cli/args.rs` | **Modify.** Replace `AgentCommandCommand`/`AgentCommandIngestSpoolArgs`/`AgentCommandWrapArgs` with `ShellUserCommand`/`ShellAgentCommand`/`ShellAgentIndexArgs`/`ShellAgentWrapArgs`; restructure `ShellCommand` to nest `User`/`Agent`. |
| `src/cli/commands/ingest.rs` | **Modify.** `parse_ingest` drops the `"agent-command"` top-level match arm (folds into `"shell"`), keeps a legacy alias arm for the pre-rename grammar. |
| `src/cli/parse_command_log.rs` | **Modify.** `parse_shell_command` now dispatches `user`/`agent`; add `parse_shell_agent_command` (canonical) and `parse_shell_agent_command_legacy` (back-compat: `ingest-spool`/`wrap` without the `user`/`agent` prefix). |
| `src/cli/dispatch_command_log.rs` | **Modify.** Rename `run_agent_command_ingest_spool` → split into `run_shell_agent_index_local` / `run_shell_agent_index_remote`; rename `run_agent_command_wrap` → `run_shell_agent_wrap`. |
| `src/cli/run.rs` | **Modify.** Update the `IngestCommand::Shell` match arm for the new nested enum shape. |
| `src/main.rs` | **Modify.** Update the early-dispatch branches (probe/http-flag checks) for the renamed types; add a new early branch for `ShellAgentCommand::Index` that decides local-vs-forward before `RuntimeCore`/`CliMode` is constructed; replace `SetupCommandKind::AgentCommand` with a nested `SetupCommandKind::Shell(ShellSetupCommand)` (`Agent`/`Completions` variants), parsing `setup shell agent ...` / `setup shell completions ...` as two tokens; wire the new `agent_command_router()` into the merged app router. |
| `src/cli.rs` | **Modify.** Update the `dispatch_command_log` re-export list. |
| `src/cli/help.rs` | **Modify.** Update `CommandDoc`/`NestedCommandDoc` entries for `ingest shell`/`ingest agent-command` → `ingest shell user`/`ingest shell agent`; update `setup agent-command` → `setup shell agent`; add `setup shell completions`. |
| `src/surfaces.rs` | **Modify.** Update the `cli!("agent-command", ...)` registry entry's `replace:` text. |
| `src/cli/color.rs` | **Modify.** Update a stale comment referencing the old grammar (cosmetic, no behavior change). |
| `src/command_log.rs` | **Modify.** Update `is_agent_command_ingest_spool_invocation`'s tolerated argv shapes (add the new canonical shape, keep both older ones); extract `parse_agent_command_spool_lines` and `import_agent_command_records` out of `import_agent_command_spool` for reuse; add `forward_agent_command_spool` (client-side forward-and-truncate). |
| `src/agent_command_ingest.rs` | **Create.** Server-side `POST /v1/agent-commands` router/handler, mirroring `src/heartbeat.rs`'s auth/body-limit/insert pattern. |
| `src/runtime.rs` | **Modify.** Add `RuntimeCore::agent_command_router()`, mirroring `heartbeat_router()`. |
| `src/lib.rs` | **Modify.** Add `pub mod agent_command_ingest;`. |
| `src/setup.rs` | **Modify.** Rename `AgentCommandAction` → `ShellAgentAction` (and its `as_str()` strings); add `ShellCompletionsAction`; register new `shell_completions` submodule; re-export `run_shell_completions_setup`. |
| `src/setup/agent_command.rs` → `src/setup/shell_agent.rs` | **Rename + modify.** Same Install/Remove/Check phases, regenerated wrapper script text (`ingest shell agent wrap`), renamed public function `run_shell_agent_setup`. |
| `src/setup/shell_completions.rs` | **Create.** Install/Remove/Check phases for writing the zsh completion script to `~/.local/share/cortex/completions/_cortex`, mirroring `agent_command.rs`'s phase structure. |
| `src/setup/doctor.rs` | **Modify.** Add a stale agent-command-grammar systemd-unit scan (`--fix` disables flagged units). |
| `src/cli/completions.rs` | **Modify.** No parsing changes; exposed for the new setup module to reuse `zsh_completion_script()`. |
| Test sidecars for every file above | **Modify/Create.** One per modified/created source file, per this repo's `#[path = "..._tests.rs"]` convention. |

---

## Phase 1 — Rename `agent-command`/`ingest-spool` to `shell agent`/`index`

### Task 1: Restructure `ShellCommand`/`AgentCommandCommand` types in `args.rs`

**Files:**
- Modify: `src/cli/args.rs:143-190` (the `ShellCommand`, `AgentCommandCommand`, `AgentCommandIngestSpoolArgs`, `AgentCommandWrapArgs` block) and `src/cli/args.rs:100-103` (`IngestCommand` enum)
- Test: no dedicated test file for `args.rs` itself (it's plain data types) — covered by Task 2/3's parser tests

**Interfaces:**
- Consumes: nothing new
- Produces: `ShellCommand::User(ShellUserCommand)` / `ShellCommand::Agent(ShellAgentCommand)`; `ShellUserCommand::Index(ShellIndexArgs)` / `ShellUserCommand::AtuinIndex(ShellAtuinIndexArgs)`; `ShellAgentCommand::Index(ShellAgentIndexArgs)` / `ShellAgentCommand::Wrap(ShellAgentWrapArgs)`; `ShellAgentIndexArgs { path: String, json: bool, server: Option<String>, token: Option<String> }`; `ShellAgentWrapArgs { spool: String, command: Vec<String>, probe: bool }` (identical shape to the old `AgentCommandWrapArgs`, renamed). Later tasks depend on these exact names.

- [ ] **Step 1: Edit `IngestCommand` to drop the `AgentCommand` variant**

In `src/cli/args.rs`, change:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IngestCommand {
    Shell(ShellCommand),
    AgentCommand(AgentCommandCommand),
    Inventory(InventoryCommand),
    FileTail(FileTailCommand),
    SyslogStatus(OutputArgs),
    DockerStatus(OutputArgs),
    DockerSources(OutputArgs),
}
```

to:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IngestCommand {
    Shell(ShellCommand),
    Inventory(InventoryCommand),
    FileTail(FileTailCommand),
    SyslogStatus(OutputArgs),
    DockerStatus(OutputArgs),
    DockerSources(OutputArgs),
}
```

- [ ] **Step 2: Replace `ShellCommand`/`AgentCommandCommand` and their arg structs**

Change:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellCommand {
    Index(ShellIndexArgs),
    AtuinIndex(ShellAtuinIndexArgs),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentCommandCommand {
    IngestSpool(AgentCommandIngestSpoolArgs),
    Wrap(AgentCommandWrapArgs),
}
```

to:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellCommand {
    User(ShellUserCommand),
    Agent(ShellAgentCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellUserCommand {
    Index(ShellIndexArgs),
    AtuinIndex(ShellAtuinIndexArgs),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellAgentCommand {
    Index(ShellAgentIndexArgs),
    Wrap(ShellAgentWrapArgs),
}
```

- [ ] **Step 3: Replace `AgentCommandIngestSpoolArgs`/`AgentCommandWrapArgs`**

Change:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentCommandIngestSpoolArgs {
    pub path: String,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentCommandWrapArgs {
    pub spool: String,
    pub command: Vec<String>,
    /// Liveness probe used by the generated shell wrapper: when true, the command
    /// resolves and exits 0 without reading the spool or running anything, so the
    /// wrapper can confirm this subcommand path is runnable before delegating.
    pub probe: bool,
}
```

to:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellAgentIndexArgs {
    pub path: String,
    pub json: bool,
    /// Remote Cortex base URL to forward the spool to instead of writing
    /// locally. Populated from `--server` directly or from the `--server`
    /// global flag if the command doesn't set its own.
    pub server: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellAgentWrapArgs {
    pub spool: String,
    pub command: Vec<String>,
    /// Liveness probe used by the generated shell wrapper: when true, the command
    /// resolves and exits 0 without reading the spool or running anything, so the
    /// wrapper can confirm this subcommand path is runnable before delegating.
    pub probe: bool,
}
```

- [ ] **Step 4: Build to confirm the expected breakage**

Run: `cargo build --lib 2>&1 | head -60`
Expected: FAIL — multiple "cannot find type `AgentCommandCommand`", "no variant `AgentCommand`" errors across `src/cli/commands/ingest.rs`, `src/cli/parse_command_log.rs`, `src/cli/dispatch_command_log.rs`, `src/cli/run.rs`, `src/main.rs`. This confirms the type change took effect; Tasks 2–5 fix each call site.

- [ ] **Step 5: Commit**

```bash
git add src/cli/args.rs
git commit -m "refactor(cli): restructure ShellCommand into User/Agent variants"
```

---

### Task 2: Rewrite `parse_ingest` and `parse_shell_command`/`parse_agent_command_command`

**Files:**
- Modify: `src/cli/commands/ingest.rs:1-38`
- Modify: `src/cli/parse_command_log.rs` (the `parse_agent_command_command` function and the block below it)
- Test: `src/cli/commands/ingest_tests.rs`, `src/cli/parse_command_log_tests.rs`

**Interfaces:**
- Consumes: `ShellCommand`, `ShellUserCommand`, `ShellAgentCommand`, `ShellAgentIndexArgs`, `ShellAgentWrapArgs` from Task 1
- Produces: `parse_shell_command(&[String]) -> Result<ShellCommand>` (now dispatches `user`/`agent`); `parse_shell_agent_command(&[String]) -> Result<ShellAgentCommand>` (canonical `index`/`wrap`); `parse_shell_agent_command_legacy(&[String]) -> Result<ShellAgentCommand>` (back-compat `ingest-spool`/`wrap`) — Task 3 (`dispatch_command_log.rs`) and Task 5 (`main.rs`) call these by name.

- [ ] **Step 1: Write the failing tests first**

Replace the body of `src/cli/commands/ingest_tests.rs` with:

```rust
use super::*;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn parse_ingest_shell_user_and_agent_inventory_and_file_tail() {
    let shell_user = parse_ingest(&strings(&["shell", "user", "index", "--path", "/tmp/history"]))
        .unwrap();
    assert!(matches!(
        shell_user,
        CliCommand::Ingest(IngestCommand::Shell(super::super::super::ShellCommand::User(
            super::super::super::ShellUserCommand::Index(_)
        )))
    ));

    let shell_agent = parse_ingest(&strings(&[
        "shell",
        "agent",
        "index",
        "--path",
        "/tmp/spool.jsonl",
        "--json",
    ]))
    .unwrap();
    assert!(matches!(
        shell_agent,
        CliCommand::Ingest(IngestCommand::Shell(super::super::super::ShellCommand::Agent(
            super::super::super::ShellAgentCommand::Index(_)
        )))
    ));

    let inventory = parse_ingest(&strings(&["inventory", "status", "--json"])).unwrap();
    assert!(matches!(
        inventory,
        CliCommand::Ingest(IngestCommand::Inventory(
            super::super::super::InventoryCommand::Status(_)
        ))
    ));

    let file_tail = parse_ingest(&strings(&["file-tail", "list", "--json"])).unwrap();
    assert!(matches!(
        file_tail,
        CliCommand::Ingest(IngestCommand::FileTail(
            super::super::super::FileTailCommand::List(_)
        ))
    ));
}

#[test]
fn parse_ingest_accepts_legacy_agent_command_grammar() {
    let legacy = parse_ingest(&strings(&[
        "agent-command",
        "ingest-spool",
        "--path",
        "/tmp/spool.jsonl",
    ]))
    .unwrap();
    assert!(matches!(
        legacy,
        CliCommand::Ingest(IngestCommand::Shell(super::super::super::ShellCommand::Agent(
            super::super::super::ShellAgentCommand::Index(_)
        )))
    ));
}

#[test]
fn parse_ingest_syslog_and_docker_read_only_modes() {
    assert!(matches!(
        parse_ingest(&strings(&["syslog", "status", "--json"])).unwrap(),
        CliCommand::Ingest(IngestCommand::SyslogStatus(args)) if args.json
    ));
    assert!(matches!(
        parse_ingest(&strings(&["docker", "status"])).unwrap(),
        CliCommand::Ingest(IngestCommand::DockerStatus(_))
    ));
    assert!(matches!(
        parse_ingest(&strings(&["docker", "sources", "--json"])).unwrap(),
        CliCommand::Ingest(IngestCommand::DockerSources(args)) if args.json
    ));
}

#[test]
fn parse_ingest_syslog_test_is_deferred() {
    let err = parse_ingest(&strings(&["syslog", "test"]))
        .unwrap_err()
        .to_string();

    assert!(err.contains("deferred"), "got: {err}");
}
```

- [ ] **Step 2: Run the ingest tests to verify they fail**

Run: `cargo test --lib cli::commands::ingest_tests -- --nocapture`
Expected: FAIL to compile — `parse_ingest_shell_user_and_agent_inventory_and_file_tail` and `parse_ingest_accepts_legacy_agent_command_grammar` reference `ShellUserCommand`/`ShellAgentCommand` variants that `parse_ingest` doesn't yet route to.

- [ ] **Step 3: Rewrite `parse_ingest` in `src/cli/commands/ingest.rs`**

Replace:

```rust
pub(crate) fn parse_ingest(args: &[String]) -> Result<CliCommand> {
    let (domain, rest) = args.split_first().ok_or_else(|| {
        anyhow!(
            "ingest requires a subcommand (shell|agent-command|inventory|file-tail|syslog|docker)"
        )
    })?;
    let command = match domain.as_str() {
        "shell" => {
            IngestCommand::Shell(super::super::parse_command_log::parse_shell_command(rest)?)
        }
        "agent-command" => IngestCommand::AgentCommand(
            super::super::parse_command_log::parse_agent_command_command(rest)?,
        ),
        "inventory" => IngestCommand::Inventory(parse_inventory_command(rest)?),
        "file-tail" => IngestCommand::FileTail(super::file_tails::parse_file_tail_command(rest)?),
        "syslog" => parse_syslog(rest)?,
        "docker" => parse_docker(rest)?,
        _ => bail!(
            "{}",
            super::super::suggest::unknown_command(
                "ingest subcommand",
                domain,
                &[
                    "shell",
                    "agent-command",
                    "inventory",
                    "file-tail",
                    "syslog",
                    "docker",
                ],
            )
        ),
    };
    Ok(CliCommand::Ingest(command))
}
```

with:

```rust
pub(crate) fn parse_ingest(args: &[String]) -> Result<CliCommand> {
    let (domain, rest) = args.split_first().ok_or_else(|| {
        anyhow!("ingest requires a subcommand (shell|inventory|file-tail|syslog|docker)")
    })?;
    let command = match domain.as_str() {
        "shell" => {
            IngestCommand::Shell(super::super::parse_command_log::parse_shell_command(rest)?)
        }
        // Back-compat: pre-restructure grammar `ingest agent-command {ingest-spool|wrap}`.
        // Keep accepting this until every deployed wrapper/timer has been
        // regenerated via `cortex setup shell agent install` (see bead
        // syslog-mcp-4n4a6, which this alias exists specifically to prevent
        // recurring).
        "agent-command" => IngestCommand::Shell(super::super::ShellCommand::Agent(
            super::super::parse_command_log::parse_shell_agent_command_legacy(rest)?,
        )),
        "inventory" => IngestCommand::Inventory(parse_inventory_command(rest)?),
        "file-tail" => IngestCommand::FileTail(super::file_tails::parse_file_tail_command(rest)?),
        "syslog" => parse_syslog(rest)?,
        "docker" => parse_docker(rest)?,
        _ => bail!(
            "{}",
            super::super::suggest::unknown_command(
                "ingest subcommand",
                domain,
                &["shell", "inventory", "file-tail", "syslog", "docker"],
            )
        ),
    };
    Ok(CliCommand::Ingest(command))
}
```

- [ ] **Step 4: Rewrite `parse_shell_command` and the agent-command parsers in `src/cli/parse_command_log.rs`**

Replace:

```rust
use super::{
    AgentCommandCommand, AgentCommandIngestSpoolArgs, AgentCommandWrapArgs, ShellAtuinIndexArgs,
    ShellCommand, ShellIndexArgs,
};

pub(crate) fn parse_shell_command(args: &[String]) -> Result<ShellCommand> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("shell subcommand is required"))?;
    match command.as_str() {
        "index" => parse_shell_index(rest),
        "atuin-index" => parse_shell_atuin_index(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command("shell subcommand", command, &["index", "atuin-index"])
        ),
    }
}

pub(crate) fn parse_agent_command_command(args: &[String]) -> Result<AgentCommandCommand> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("agent-command subcommand is required"))?;
    match command.as_str() {
        "ingest-spool" => parse_agent_command_ingest_spool(rest),
        "wrap" => parse_agent_command_wrap(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command(
                "agent-command subcommand",
                command,
                &["ingest-spool", "wrap"],
            )
        ),
    }
}
```

with:

```rust
use super::{
    ShellAgentCommand, ShellAgentIndexArgs, ShellAgentWrapArgs, ShellAtuinIndexArgs, ShellCommand,
    ShellIndexArgs, ShellUserCommand,
};

pub(crate) fn parse_shell_command(args: &[String]) -> Result<ShellCommand> {
    let (group, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("shell requires a subcommand (user|agent)"))?;
    match group.as_str() {
        "user" => Ok(ShellCommand::User(parse_shell_user_command(rest)?)),
        "agent" => Ok(ShellCommand::Agent(parse_shell_agent_command(rest)?)),
        _ => bail!(
            "{}",
            super::suggest::unknown_command("shell subcommand", group, &["user", "agent"])
        ),
    }
}

fn parse_shell_user_command(args: &[String]) -> Result<ShellUserCommand> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("shell user subcommand is required"))?;
    match command.as_str() {
        "index" => parse_shell_index(rest),
        "atuin-index" => parse_shell_atuin_index(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command(
                "shell user subcommand",
                command,
                &["index", "atuin-index"],
            )
        ),
    }
}

pub(crate) fn parse_shell_agent_command(args: &[String]) -> Result<ShellAgentCommand> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("shell agent subcommand is required"))?;
    match command.as_str() {
        "index" => parse_shell_agent_index(rest),
        "wrap" => parse_shell_agent_wrap(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command("shell agent subcommand", command, &["index", "wrap"])
        ),
    }
}

/// Back-compat shim for the pre-restructure grammar: `ingest agent-command
/// {ingest-spool|wrap}`. `ingest-spool` maps to the same `Index` variant as
/// the canonical `index` verb.
pub(crate) fn parse_shell_agent_command_legacy(args: &[String]) -> Result<ShellAgentCommand> {
    let (command, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("agent-command subcommand is required"))?;
    match command.as_str() {
        "ingest-spool" => parse_shell_agent_index(rest),
        "wrap" => parse_shell_agent_wrap(rest),
        _ => bail!(
            "{}",
            super::suggest::unknown_command(
                "agent-command subcommand",
                command,
                &["ingest-spool", "wrap"],
            )
        ),
    }
}
```

- [ ] **Step 5: Replace `parse_agent_command_ingest_spool`/`parse_agent_command_wrap` with the renamed, `--server`/`--token`-aware versions**

Replace:

```rust
fn parse_agent_command_ingest_spool(args: &[String]) -> Result<AgentCommandCommand> {
    let mut path = None;
    let mut json = false;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                i += 1;
                path = Some(required_value(args, i, "--path")?);
            }
            "--json" => json = true,
            other => bail!("unknown agent-command ingest-spool argument: {other}"),
        }
        i += 1;
    }
    let path =
        path.ok_or_else(|| anyhow::anyhow!("agent-command ingest-spool requires --path PATH"))?;
    Ok(AgentCommandCommand::IngestSpool(
        AgentCommandIngestSpoolArgs { path, json },
    ))
}

fn parse_agent_command_wrap(args: &[String]) -> Result<AgentCommandCommand> {
    let mut spool = None;
    let mut probe = false;
    let mut command_start = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--spool" => {
                i += 1;
                spool = Some(required_value(args, i, "--spool")?);
            }
            "--probe" => {
                probe = true;
            }
            "--" => {
                command_start = Some(i + 1);
                break;
            }
            other => bail!("unknown agent-command wrap argument: {other}"),
        }
        i += 1;
    }
    // A probe is a liveness check the generated wrapper runs before delegating;
    // it needs neither a spool nor a command.
    if probe {
        return Ok(AgentCommandCommand::Wrap(AgentCommandWrapArgs {
            spool: spool.unwrap_or_default(),
            command: Vec::new(),
            probe: true,
        }));
    }
    let spool = spool.ok_or_else(|| anyhow::anyhow!("agent-command wrap requires --spool PATH"))?;
    let start =
        command_start.ok_or_else(|| anyhow::anyhow!("agent-command wrap requires -- COMMAND"))?;
    let command = args[start..].to_vec();
    if command.is_empty() {
        bail!("agent-command wrap requires COMMAND after --");
    }
    Ok(AgentCommandCommand::Wrap(AgentCommandWrapArgs {
        spool,
        command,
        probe: false,
    }))
}
```

with:

```rust
fn parse_shell_agent_index(args: &[String]) -> Result<ShellAgentCommand> {
    let mut path = None;
    let mut json = false;
    let mut server = None;
    let mut token = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                i += 1;
                path = Some(required_value(args, i, "--path")?);
            }
            "--json" => json = true,
            "--server" => {
                i += 1;
                server = Some(required_value(args, i, "--server")?);
            }
            "--token" => {
                i += 1;
                token = Some(required_value(args, i, "--token")?);
            }
            other => bail!("unknown shell agent index argument: {other}"),
        }
        i += 1;
    }
    let path = path.ok_or_else(|| anyhow::anyhow!("shell agent index requires --path PATH"))?;
    Ok(ShellAgentCommand::Index(ShellAgentIndexArgs {
        path,
        json,
        server,
        token,
    }))
}

fn parse_shell_agent_wrap(args: &[String]) -> Result<ShellAgentCommand> {
    let mut spool = None;
    let mut probe = false;
    let mut command_start = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--spool" => {
                i += 1;
                spool = Some(required_value(args, i, "--spool")?);
            }
            "--probe" => {
                probe = true;
            }
            "--" => {
                command_start = Some(i + 1);
                break;
            }
            other => bail!("unknown shell agent wrap argument: {other}"),
        }
        i += 1;
    }
    // A probe is a liveness check the generated wrapper runs before delegating;
    // it needs neither a spool nor a command.
    if probe {
        return Ok(ShellAgentCommand::Wrap(ShellAgentWrapArgs {
            spool: spool.unwrap_or_default(),
            command: Vec::new(),
            probe: true,
        }));
    }
    let spool = spool.ok_or_else(|| anyhow::anyhow!("shell agent wrap requires --spool PATH"))?;
    let start =
        command_start.ok_or_else(|| anyhow::anyhow!("shell agent wrap requires -- COMMAND"))?;
    let command = args[start..].to_vec();
    if command.is_empty() {
        bail!("shell agent wrap requires COMMAND after --");
    }
    Ok(ShellAgentCommand::Wrap(ShellAgentWrapArgs {
        spool,
        command,
        probe: false,
    }))
}
```

- [ ] **Step 6: Rewrite `src/cli/parse_command_log_tests.rs`'s agent-command tests**

Replace the three tests `parses_agent_command_ingest_spool`, `parses_agent_command_wrap_after_separator`, `parses_agent_command_wrap_probe_without_spool_or_command` with:

```rust
#[test]
fn parses_shell_agent_index() {
    let args = vec![
        "index".to_string(),
        "--path".to_string(),
        "/tmp/commands.jsonl".to_string(),
    ];

    let command = parse_shell_agent_command(&args).unwrap();

    match command {
        ShellAgentCommand::Index(args) => {
            assert_eq!(args.path, "/tmp/commands.jsonl");
            assert!(!args.json);
            assert!(args.server.is_none());
            assert!(args.token.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_shell_agent_index_with_server_and_token() {
    let args = vec![
        "index".to_string(),
        "--path".to_string(),
        "/tmp/commands.jsonl".to_string(),
        "--server".to_string(),
        "https://cortex.example.test".to_string(),
        "--token".to_string(),
        "secret".to_string(),
    ];

    let command = parse_shell_agent_command(&args).unwrap();

    match command {
        ShellAgentCommand::Index(args) => {
            assert_eq!(args.server.as_deref(), Some("https://cortex.example.test"));
            assert_eq!(args.token.as_deref(), Some("secret"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_legacy_agent_command_ingest_spool_as_shell_agent_index() {
    let args = vec![
        "ingest-spool".to_string(),
        "--path".to_string(),
        "/tmp/commands.jsonl".to_string(),
    ];

    let command = parse_shell_agent_command_legacy(&args).unwrap();

    match command {
        ShellAgentCommand::Index(args) => {
            assert_eq!(args.path, "/tmp/commands.jsonl");
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_shell_agent_wrap_after_separator() {
    let args = vec![
        "wrap".to_string(),
        "--spool".to_string(),
        "/tmp/commands.jsonl".to_string(),
        "--".to_string(),
        "echo".to_string(),
        "hello".to_string(),
    ];

    let command = parse_shell_agent_command(&args).unwrap();

    match command {
        ShellAgentCommand::Wrap(args) => {
            assert_eq!(args.spool, "/tmp/commands.jsonl");
            assert_eq!(args.command, vec!["echo", "hello"]);
            assert!(!args.probe);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_shell_agent_wrap_probe_without_spool_or_command() {
    let args = vec!["wrap".to_string(), "--probe".to_string()];

    let command = parse_shell_agent_command(&args).unwrap();

    match command {
        ShellAgentCommand::Wrap(args) => {
            assert!(args.probe);
            assert!(args.command.is_empty());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
```

- [ ] **Step 7: Run the two updated test modules**

Run: `cargo test --lib cli::commands::ingest_tests cli::parse_command_log_tests -- --nocapture`
Expected: still FAIL to compile (dispatch/run/main.rs haven't been updated yet — Tasks 3–5 finish the chain). Confirm the *specific* errors are now only in `dispatch_command_log.rs`, `run.rs`, and `main.rs`, not in the two files just edited.

- [ ] **Step 8: Commit**

```bash
git add src/cli/commands/ingest.rs src/cli/commands/ingest_tests.rs src/cli/parse_command_log.rs src/cli/parse_command_log_tests.rs
git commit -m "refactor(cli): parse ingest shell user/agent grammar with legacy alias"
```

---

### Task 3: Rewrite `dispatch_command_log.rs` for the renamed args + local/remote split

**Files:**
- Modify: `src/cli/dispatch_command_log.rs`
- Test: `src/cli/dispatch_command_log_tests.rs`

**Interfaces:**
- Consumes: `ShellAgentIndexArgs`, `ShellAgentWrapArgs` from Task 1; `command_log::forward_agent_command_spool` from Task 8 (Phase 3) — **stub it in this task** (see Step 3) so Phase 1 compiles and is independently testable before Phase 3 lands.
- Produces: `run_shell_agent_index_local(mode: &CliMode, args: ShellAgentIndexArgs) -> Result<()>`; `run_shell_agent_index_remote(args: ShellAgentIndexArgs, server: String) -> Result<()>`; `run_shell_agent_wrap(args: ShellAgentWrapArgs) -> Result<i32>` — Task 4 (`run.rs`) and Task 5 (`main.rs`) call these by name.

- [ ] **Step 1: Read the current test file to know what to preserve**

Run: `cat src/cli/dispatch_command_log_tests.rs`
Expected: shows existing tests for `run_shell_index`/`run_shell_atuin_index`/`run_agent_command_ingest_spool`/`run_agent_command_wrap`. Keep the shell-user tests as-is; the agent-command tests get renamed in Step 4 below.

- [ ] **Step 2: Add a temporary local stub for `forward_agent_command_spool` so Phase 1 compiles standalone**

In `src/command_log.rs`, add this stub function near `run_agent_command_wrapper` (Phase 3, Task 8 replaces the body with the real implementation — this task only needs the signature to exist so `dispatch_command_log.rs` compiles):

```rust
/// Forwards a local agent-command spool to a remote Cortex instead of
/// writing to a local `DbPool`. Real implementation lands in Phase 3 (Task
/// 8) — this stub exists so Phase 1's CLI rename compiles independently.
pub async fn forward_agent_command_spool(
    _path: &Path,
    _target: &str,
    _token: Option<&str>,
) -> Result<CommandLogImportResult> {
    anyhow::bail!("forwarding not yet implemented")
}
```

- [ ] **Step 3: Rewrite `dispatch_command_log.rs`**

Replace the whole file with:

```rust
use std::path::PathBuf;

use anyhow::{Result, bail};
use cortex::command_log::{self, CommandLogImportResult};

use super::{
    CliMode, ShellAgentIndexArgs, ShellAgentWrapArgs, ShellAtuinIndexArgs, ShellIndexArgs,
};

pub(crate) async fn run_shell_index(mode: &CliMode, args: ShellIndexArgs) -> Result<()> {
    let CliMode::Local(service) = mode else {
        bail!("shell user index is local-only; run without --http/--server/--token");
    };
    let result = service
        .import_shell_history(PathBuf::from(args.path), args.shell)
        .await?;
    print_import_result("shell user index", &result, args.json)
}

pub(crate) async fn run_shell_atuin_index(mode: &CliMode, args: ShellAtuinIndexArgs) -> Result<()> {
    let CliMode::Local(service) = mode else {
        bail!("shell user atuin-index is local-only; run without --http/--server/--token");
    };
    let result = service
        .import_atuin_history(PathBuf::from(args.path))
        .await?;
    print_import_result("shell user atuin-index", &result, args.json)
}

pub(crate) async fn run_shell_agent_index_local(
    mode: &CliMode,
    args: ShellAgentIndexArgs,
) -> Result<()> {
    let CliMode::Local(service) = mode else {
        bail!("shell agent index is local-only without --server; pass --server URL to forward");
    };
    let result = service
        .import_agent_command_spool(PathBuf::from(args.path))
        .await?;
    print_import_result("shell agent index", &result, args.json)
}

pub(crate) async fn run_shell_agent_index_remote(
    args: ShellAgentIndexArgs,
    server: String,
) -> Result<()> {
    let result = command_log::forward_agent_command_spool(
        std::path::Path::new(&args.path),
        &server,
        args.token.as_deref(),
    )
    .await?;
    print_import_result("shell agent index (forwarded)", &result, args.json)
}

pub(crate) fn run_shell_agent_wrap(args: ShellAgentWrapArgs) -> Result<i32> {
    command_log::run_agent_command_wrapper(PathBuf::from(args.spool).as_path(), &args.command)
}

fn print_import_result(label: &str, result: &CommandLogImportResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!("{label}");
        println!("scanned: {}", result.scanned);
        println!("imported: {}", result.imported);
        println!("skipped: {}", result.skipped);
        println!("skipped_duplicates: {}", result.skipped_duplicates);
        println!("errors: {}", result.errors);
    }
    Ok(())
}

#[cfg(test)]
#[path = "dispatch_command_log_tests.rs"]
mod tests;
```

- [ ] **Step 4: Update `dispatch_command_log_tests.rs`**

Rename any test function/reference from `run_agent_command_ingest_spool` to `run_shell_agent_index_local`, and from `run_agent_command_wrap` to `run_shell_agent_wrap`, updating the arg-struct names (`AgentCommandIngestSpoolArgs` → `ShellAgentIndexArgs` with the two new `server`/`token: None` fields added to every struct literal, `AgentCommandWrapArgs` → `ShellAgentWrapArgs`) to match Task 1's renamed types. Keep assertions identical otherwise — only names change.

- [ ] **Step 5: Run this module's tests**

Run: `cargo test --lib cli::dispatch_command_log_tests -- --nocapture`
Expected: still FAIL to compile — `run.rs` and `main.rs` (Tasks 4–5) haven't been updated, so the crate as a whole won't build yet. Confirm no NEW errors originate from this file or its test sidecar.

- [ ] **Step 6: Commit**

```bash
git add src/cli/dispatch_command_log.rs src/cli/dispatch_command_log_tests.rs src/command_log.rs
git commit -m "refactor(cli): split shell agent index dispatch into local/remote paths"
```

---

### Task 4: Update `run.rs`'s `IngestCommand::Shell` match arm and `cli.rs`'s re-exports

**Files:**
- Modify: `src/cli/run.rs:69-92`
- Modify: `src/cli.rs:41`
- Test: `src/cli/run_tests.rs`

**Interfaces:**
- Consumes: `ShellCommand::User`/`ShellCommand::Agent`, `ShellUserCommand`, `ShellAgentCommand` from Task 1; `run_shell_agent_index_local`/`run_shell_agent_wrap` from Task 3.
- Produces: nothing new — this is the last CLI-internal wiring hop before `main.rs` (Task 5).

- [ ] **Step 1: Replace the `IngestCommand::Shell`/`IngestCommand::AgentCommand` match arms in `run.rs`**

Replace:

```rust
            IngestCommand::Shell(shell) => match shell {
                ShellCommand::Index(args) => {
                    super::dispatch_command_log::run_shell_index(&mode, args).await
                }
                ShellCommand::AtuinIndex(args) => {
                    super::dispatch_command_log::run_shell_atuin_index(&mode, args).await
                }
            },
            IngestCommand::AgentCommand(command) => match command {
                AgentCommandCommand::IngestSpool(args) => {
                    super::dispatch_command_log::run_agent_command_ingest_spool(&mode, args).await
                }
                AgentCommandCommand::Wrap(_) => {
                    bail!(
                        "internal: ingest agent-command wrap must be dispatched before CliMode creation"
                    )
                }
            },
```

with:

```rust
            IngestCommand::Shell(shell) => match shell {
                ShellCommand::User(user) => match user {
                    ShellUserCommand::Index(args) => {
                        super::dispatch_command_log::run_shell_index(&mode, args).await
                    }
                    ShellUserCommand::AtuinIndex(args) => {
                        super::dispatch_command_log::run_shell_atuin_index(&mode, args).await
                    }
                },
                // Both `ShellAgentCommand` variants are intercepted in
                // `main.rs` before `CliMode`/`RuntimeCore` construction —
                // `Index` because it may forward instead of touching a local
                // DB, `Wrap` because of its liveness-probe fast path. Neither
                // should ever reach this generic dispatcher.
                ShellCommand::Agent(_) => {
                    bail!(
                        "internal: ingest shell agent commands must be dispatched before CliMode creation"
                    )
                }
            },
```

- [ ] **Step 2: Update the `use` list at the top of `run.rs`**

Find the import line (around line 5):

```rust
    AgentCommandCommand, AlertsCommand, CliCommand, DbCommand, GraphCommand, IngestCommand,
```

Replace with:

```rust
    AlertsCommand, CliCommand, DbCommand, GraphCommand, IngestCommand, ShellAgentCommand,
    ShellCommand, ShellUserCommand,
```

(keep whatever other names were already on that `use` line — only remove `AgentCommandCommand` and add the three new type names; check the full original `use` block with `sed -n '1,15p' src/cli/run.rs` before editing so no existing import is dropped.)

- [ ] **Step 3: Update `cli.rs`'s re-export**

Replace:

```rust
pub(crate) use dispatch_command_log::run_agent_command_wrap;
```

with:

```rust
pub(crate) use dispatch_command_log::{
    run_shell_agent_index_local, run_shell_agent_index_remote, run_shell_agent_wrap,
};
```

- [ ] **Step 4: Build**

Run: `cargo build --lib 2>&1 | head -60`
Expected: FAIL only in `src/main.rs` now (its `AgentCommand`/`AgentCommandCommand` pattern matches haven't been updated — Task 5 fixes this). Confirm `run.rs` and `cli.rs` no longer appear in the error list.

- [ ] **Step 5: Commit**

```bash
git add src/cli/run.rs src/cli.rs
git commit -m "refactor(cli): dispatch shell user/agent commands in run.rs"
```

---

### Task 5: Update `main.rs` dispatch branches, `SetupCommandKind`, and router mount

**Files:**
- Modify: `src/main.rs` (four regions: the `Wrap` probe branch ~line 119, the local-only `matches!` branch ~line 155, `SetupCommandKind::AgentCommand` definition ~line 560 and its parse block ~line 762, the router-merge block ~line 450)
- Test: `src/main_tests.rs`

**Interfaces:**
- Consumes: `run_shell_agent_index_local`/`run_shell_agent_index_remote`/`run_shell_agent_wrap` from Task 3/4; `ShellCommand::Agent`, `ShellAgentCommand::Index`/`Wrap` from Task 1; `cortex::setup::ShellAgentAction` from Task 6 (Phase 1 continues into setup rename).
- Produces: nothing new for later CLI tasks — this closes out Phase 1's CLI dispatch chain. The `ShellSetupCommand::Completions` variant added here is consumed by Phase 2 (Task 10).

- [ ] **Step 1: Replace the `Wrap` probe branch**

Replace:

```rust
    if let cli::CliCommand::Ingest(cli::IngestCommand::AgentCommand(
        cli::AgentCommandCommand::Wrap(args),
    )) = command
    {
        // Liveness probe from the generated wrapper: succeed fast, run nothing.
        // Keep this ahead of the http-flag check so a probe never errors.
        if args.probe {
            std::process::exit(0);
        }
        if let Some(trigger) = flags.http_flag_trigger() {
            anyhow::bail!(
                "{} has no effect on `ingest agent-command wrap` (wrapper command); remove --http / --server / --token",
                trigger
            );
        }
        let code = cli::run_agent_command_wrap(args)?;
        std::process::exit(code);
    }
```

with:

```rust
    if let cli::CliCommand::Ingest(cli::IngestCommand::Shell(cli::ShellCommand::Agent(
        cli::ShellAgentCommand::Wrap(args),
    ))) = command
    {
        // Liveness probe from the generated wrapper: succeed fast, run nothing.
        // Keep this ahead of the http-flag check so a probe never errors.
        if args.probe {
            std::process::exit(0);
        }
        if let Some(trigger) = flags.http_flag_trigger() {
            anyhow::bail!(
                "{} has no effect on `ingest shell agent wrap` (wrapper command); remove --http / --server / --token",
                trigger
            );
        }
        let code = cli::run_shell_agent_wrap(args)?;
        std::process::exit(code);
    }

    if let cli::CliCommand::Ingest(cli::IngestCommand::Shell(cli::ShellCommand::Agent(
        cli::ShellAgentCommand::Index(mut args),
    ))) = command
    {
        if args.server.is_none() {
            args.server = flags.server.clone();
        }
        if args.token.is_none() {
            args.token = flags.token.clone();
        }
        if let Some(server) = args.server.clone() {
            return cli::run_shell_agent_index_remote(args, server).await;
        }
        if flags.force_http {
            anyhow::bail!(
                "--http requires --server URL for `ingest shell agent index`; pass --server explicitly"
            );
        }
        let runtime = RuntimeCore::load_query_only().await?;
        return cli::run_shell_agent_index_local(&cli::CliMode::Local(runtime.service()), args)
            .await;
    }
```

- [ ] **Step 2: Update the local-only `matches!` branch to cover only `ShellCommand::User`**

Replace:

```rust
    if matches!(
        command,
        cli::CliCommand::Ingest(cli::IngestCommand::Shell(_))
            | cli::CliCommand::Ingest(cli::IngestCommand::AgentCommand(
                cli::AgentCommandCommand::IngestSpool(_)
            ))
    ) {
        if let Some(trigger) = flags.http_flag_trigger() {
            anyhow::bail!(
                "{} has no effect on local agent commands; remove --http / --server / --token",
                trigger
            );
        }
        let runtime = RuntimeCore::load_query_only().await?;
        return cli::run(cli::CliMode::Local(runtime.service()), command).await;
    }
```

with:

```rust
    if matches!(
        command,
        cli::CliCommand::Ingest(cli::IngestCommand::Shell(cli::ShellCommand::User(_)))
    ) {
        if let Some(trigger) = flags.http_flag_trigger() {
            anyhow::bail!(
                "{} has no effect on local shell commands; remove --http / --server / --token",
                trigger
            );
        }
        let runtime = RuntimeCore::load_query_only().await?;
        return cli::run(cli::CliMode::Local(runtime.service()), command).await;
    }
```

- [ ] **Step 3: Rename `SetupCommandKind::AgentCommand` → nested `Shell(ShellSetupCommand)`**

Find (around line 560):

```rust
    AgentCommand(cortex::setup::AgentCommandAction),
```

Replace with:

```rust
    Shell(ShellSetupCommand),
```

Add this new enum alongside `SetupCommandKind` (same visibility/derives — check the exact derive list on `SetupCommandKind` with `sed -n '555,565p' src/main.rs` and copy it):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellSetupCommand {
    Agent(cortex::setup::ShellAgentAction),
    Completions(cortex::setup::ShellCompletionsAction),
}
```

Find the dispatch match (around line 333):

```rust
        SetupCommandKind::AgentCommand(action) => {
```

Replace with (keep whatever the original arm's body was — only the pattern changes; check the exact original body with `sed -n '325,345p' src/main.rs` before editing):

```rust
        SetupCommandKind::Shell(ShellSetupCommand::Agent(action)) => {
            cortex::setup::run_shell_agent_setup(action).await?
        }
        SetupCommandKind::Shell(ShellSetupCommand::Completions(action)) => {
            cortex::setup::run_shell_completions_setup(action)?
        }
```

(the exact right-hand side for the `Agent` arm mirrors whatever `AgentCommand`'s arm currently does — it's `cortex::setup::run_agent_command_setup(action).await?` per Task 6's rename, becoming `run_shell_agent_setup`.)

- [ ] **Step 4: Replace the `Some("agent-command")` parse block with a nested `shell` block**

`setup` subcommand parsing here is intentionally becoming nested rather than flat-hyphenated for this one domain — `cortex setup shell agent install` and `cortex setup shell completions install`, mirroring the `ingest shell user`/`ingest shell agent` nesting from Task 1–2. This is a deliberate one-off: every *other* `setup` subcommand (`heartbeat-agent`, `debug-wrapper`, `sessions-index-timer`, etc.) stays flat and untouched by this plan — only the agent-command/completions surfaces move under `shell`.

Replace:

```rust
    if matches!(
        iter.clone().next().map(String::as_str),
        Some("agent-command")
    ) {
        let _ = iter.next();
        let (action, json) = parse_setup_subcommand_args("agent-command", iter)?;
        return Ok(SetupCommand {
            kind: SetupCommandKind::AgentCommand(match action {
                "install" => cortex::setup::AgentCommandAction::Install,
                "remove" => cortex::setup::AgentCommandAction::Remove,
                _ => cortex::setup::AgentCommandAction::Check,
            }),
            json,
        });
    }
```

with:

```rust
    if matches!(iter.clone().next().map(String::as_str), Some("shell")) {
        let _ = iter.next();
        match iter.next().map(String::as_str) {
            Some("agent") => {
                let (action, json) = parse_setup_subcommand_args("shell agent", iter)?;
                return Ok(SetupCommand {
                    kind: SetupCommandKind::Shell(ShellSetupCommand::Agent(match action {
                        "install" => cortex::setup::ShellAgentAction::Install,
                        "remove" => cortex::setup::ShellAgentAction::Remove,
                        _ => cortex::setup::ShellAgentAction::Check,
                    })),
                    json,
                });
            }
            Some("completions") => {
                let (action, json) = parse_setup_subcommand_args("shell completions", iter)?;
                return Ok(SetupCommand {
                    kind: SetupCommandKind::Shell(ShellSetupCommand::Completions(match action {
                        "install" => cortex::setup::ShellCompletionsAction::Install,
                        "remove" => cortex::setup::ShellCompletionsAction::Remove,
                        _ => cortex::setup::ShellCompletionsAction::Check,
                    })),
                    json,
                });
            }
            Some(other) => anyhow::bail!(
                "unknown setup shell subcommand: {other} (expected agent|completions)"
            ),
            None => anyhow::bail!("setup shell requires a subcommand (agent|completions)"),
        }
    }
```

This mirrors the exact idiom already used a few lines above for `sessions-index-timer`/`sessions-watch-service` (`let mut iter = args.iter();` is a plain `std::slice::Iter<'_, String>`; `iter.next()` consumes one token at a time, and `iter` is then handed by value into `parse_setup_subcommand_args`, which is exactly what happens here after consuming two tokens — `"shell"` then `"agent"`/`"completions"` — instead of one.

- [ ] **Step 5: Add the forwarded agent-command ingest router to the merged app (Phase 3 dependency — safe to add now since Task 7 defines it)**

Find:

```rust
    app = app.merge(runtime.heartbeat_router());
    info!("Heartbeat receiver mounted at /v1/heartbeats");
```

Add immediately after:

```rust
    app = app.merge(runtime.agent_command_router());
    info!("Agent-command forward receiver mounted at /v1/agent-commands");
```

- [ ] **Step 6: Build**

Run: `cargo build --lib --bin cortex 2>&1 | tail -60`
Expected: FAIL — `cortex::setup::ShellAgentAction`/`run_shell_agent_setup`/`ShellCompletionsAction`/`run_shell_completions_setup` and `runtime.agent_command_router()` don't exist yet (Task 6, Task 7, Task 9 create them). Confirm the error list now points only at `src/setup.rs`, `src/setup/agent_command.rs`, and `src/runtime.rs` (Phase 1's Task 6 and Phase 3's Task 7 close these out; Phase 2's Task 9 provides `ShellCompletionsAction`).

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "refactor(cli): dispatch shell agent index/wrap before CliMode construction"
```

---

### Task 6: Rename `setup agent-command` → `setup shell agent`

**Note on why this rename gets no legacy alias, unlike `ingest` (Task 2):** `ingest agent-command ingest-spool`/`wrap` needs a back-compat alias because it's embedded in unattended, already-deployed artifacts — the generated wrapper script and any manually-created systemd timer `ExecStart=` lines — that keep invoking the old grammar until something (a human, or Task 14's `doctor --fix`) regenerates or fixes them. `setup agent-command install|remove|check`, by contrast, is a command an *operator types interactively* to manage that installation; nothing on disk embeds a hardcoded call to `cortex setup agent-command ...` the way the wrapper embeds `cortex ingest agent-command wrap`. Dropping the old `setup` grammar outright (no alias) is deliberate, not an oversight: worst case, an operator who still has the old muscle memory gets a normal "unknown command" error with a suggestion, the same experience as any other CLI rename with no unattended-artifact risk.

**Files:**
- Modify: `src/setup.rs` (the `AgentCommandAction` enum/impl and module registration)
- Rename: `src/setup/agent_command.rs` → `src/setup/shell_agent.rs`; `src/setup/agent_command_tests.rs` → `src/setup/shell_agent_tests.rs`
- Test: `src/setup/shell_agent_tests.rs`

**Interfaces:**
- Consumes: nothing new
- Produces: `cortex::setup::ShellAgentAction { Install, Remove, Check }`; `cortex::setup::run_shell_agent_setup(ShellAgentAction) -> io::Result<SetupReport>` — consumed by Task 5's `main.rs` dispatch.

- [ ] **Step 1: Rename the files with git so history is preserved**

```bash
git mv src/setup/agent_command.rs src/setup/shell_agent.rs
git mv src/setup/agent_command_tests.rs src/setup/shell_agent_tests.rs
```

- [ ] **Step 2: Update `src/setup.rs`'s module list and re-exports**

Replace:

```rust
mod agent_command;
```

with:

```rust
mod shell_agent;
```

Replace:

```rust
pub use agent_command::run_agent_command_setup;
```

with:

```rust
pub use shell_agent::run_shell_agent_setup;
```

- [ ] **Step 3: Rename the `AgentCommandAction` enum and its `as_str()` impl**

Replace:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommandAction {
    Install,
    Remove,
    Check,
}
```

with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAgentAction {
    Install,
    Remove,
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCompletionsAction {
    Install,
    Remove,
    Check,
}
```

(`ShellCompletionsAction` is defined here now so Task 5's `main.rs` edit and Task 9's `shell_completions.rs` both compile against it; Task 9 fills in `run_shell_completions_setup`.)

Replace:

```rust
impl AgentCommandAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "agent-command-install",
            Self::Remove => "agent-command-remove",
            Self::Check => "agent-command-check",
        }
    }
}
```

with:

```rust
impl ShellAgentAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "shell-agent-install",
            Self::Remove => "shell-agent-remove",
            Self::Check => "shell-agent-check",
        }
    }
}

impl ShellCompletionsAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "shell-completions-install",
            Self::Remove => "shell-completions-remove",
            Self::Check => "shell-completions-check",
        }
    }
}
```

- [ ] **Step 4: Rename every `AgentCommandAction`/`run_agent_command_setup` reference inside `src/setup/shell_agent.rs`**

In the renamed file, rename:
- `pub async fn run_agent_command_setup(action: AgentCommandAction)` → `pub async fn run_shell_agent_setup(action: ShellAgentAction)`
- Every `AgentCommandAction::Install`/`::Remove`/`::Check` → `ShellAgentAction::Install`/`::Remove`/`::Check`

Rename the wrapper script's embedded CLI invocation. Find:

```rust
fn agent_command_wrapper_script(cortex_bin: &Path, spool_path: &Path) -> String {
    let cortex_bin = setup_path_value(cortex_bin).expect("validated cortex binary path");
    let spool_path = setup_path_value(spool_path).expect("validated agent command spool path");
    format!(
        r#"#!/usr/bin/env sh
# Best-effort agent-command logging. The probe confirms `ingest agent-command
# wrap` is runnable; if cortex is missing or its CLI changed, fall through and
# exec the command directly so logging can never brick the shell.
if {cortex_bin} ingest agent-command wrap --probe >/dev/null 2>&1; then
  exec {cortex_bin} ingest agent-command wrap --spool {spool_path} -- "$@"
fi
exec "$@"
"#
    )
}
```

Replace with:

```rust
fn agent_command_wrapper_script(cortex_bin: &Path, spool_path: &Path) -> String {
    let cortex_bin = setup_path_value(cortex_bin).expect("validated cortex binary path");
    let spool_path = setup_path_value(spool_path).expect("validated agent command spool path");
    format!(
        r#"#!/usr/bin/env sh
# Best-effort agent-command logging. The probe confirms `ingest shell agent
# wrap` is runnable; if cortex is missing or its CLI changed, fall through and
# exec the command directly so logging can never brick the shell.
if {cortex_bin} ingest shell agent wrap --probe >/dev/null 2>&1; then
  exec {cortex_bin} ingest shell agent wrap --spool {spool_path} -- "$@"
fi
exec "$@"
"#
    )
}
```

Every remaining `"agent-command install"`/`"run cortex setup agent-command install"` string inside this file (in `check_file_phase(...)`, `agent_command_state_phase`, error/warn messages) becomes `"run cortex setup shell agent install"`. Function names local to this file (`install_agent_command_files`, `agent_command_content_phase`, `ensure_agent_command_spool_file`, `remove_agent_command_wrapper`, `agent_command_state_phase`, `agent_command_env_phase`, `resolve_agent_command_cortex_binary`, `validate_agent_command_binary`) can keep their existing names — they're private implementation details, not user-facing grammar, so renaming them is optional polish rather than required for this plan's scope. Leave them as-is to keep the diff focused.

- [ ] **Step 5: Update `src/setup/shell_agent_tests.rs`**

Rename every `run_agent_command_setup`/`AgentCommandAction` reference to `run_shell_agent_setup`/`ShellAgentAction`. Update any string assertions that check for `"cortex setup agent-command install"` to `"cortex setup shell agent install"`, and `"ingest agent-command wrap"` to `"ingest shell agent wrap"`.

- [ ] **Step 6: Build and run this module's tests**

Run: `cargo test --lib setup::shell_agent_tests -- --nocapture`
Expected: PASS (Phase 1's remaining compile errors are in `help.rs`/`surfaces.rs`/`color.rs`/`command_log.rs`'s guard and their test sidecars — Tasks 7–8 below finish those; this module itself should build and pass standalone once Task 5's `main.rs` references compile, which requires this task's types to exist — run `cargo build --lib` first to confirm the crate-wide error list has shrunk to just those remaining files).

- [ ] **Step 7: Commit**

```bash
git add src/setup.rs src/setup/shell_agent.rs src/setup/shell_agent_tests.rs
git commit -m "refactor(setup): rename agent-command setup domain to shell-agent"
```

---

### Task 7: Update the self-ingest-loop guard in `command_log.rs`

**Files:**
- Modify: `src/command_log.rs` (the `is_agent_command_ingest_spool_invocation` function)
- Test: `src/command_log_tests.rs`

**Interfaces:**
- Consumes: nothing new
- Produces: nothing new — this closes the loop so the wrapper never double-logs an `ingest shell agent index` (or its immediately-prior grammar form) invocation.

**Engineering-review change applied to this task**: the plan originally tolerated a third, even-older "pre-move" grammar (`cortex agent-command ingest-spool`, with no `ingest` prefix at all). Simplicity review flagged this as dead code: that bare top-level form is a `MovedIntoGroupedDomain` surface in `src/surfaces.rs` — the CLI's own top-level parser rejects it with a "did you mean `ingest shell agent`" error and never executes it, so no live process can ever actually invoke `cortex agent-command ingest-spool` for this guard to catch. Tolerating it here added a `matches!` arm and a test case for a shape that's provably unreachable. Dropped — this guard now tolerates exactly two shapes: the canonical grammar and the one immediately-prior grouped grammar that real, already-deployed wrapper scripts on hosts like `devhost` actually still emit.

- [ ] **Step 1: Write the failing test**

In `src/command_log_tests.rs`, find `agent_command_ingest_spool_guard_is_argv_scoped` and replace it with:

```rust
#[test]
fn agent_command_ingest_spool_guard_is_argv_scoped() {
    // Canonical grammar: `cortex ingest shell agent index`.
    assert!(is_agent_command_ingest_spool_invocation(&[
        "cortex".to_string(),
        "ingest".to_string(),
        "shell".to_string(),
        "agent".to_string(),
        "index".to_string(),
    ]));
    assert!(is_agent_command_ingest_spool_invocation(&[
        "/usr/local/bin/cortex".to_string(),
        "ingest".to_string(),
        "shell".to_string(),
        "agent".to_string(),
        "index".to_string(),
        "--path".to_string(),
        "/tmp/x.jsonl".to_string(),
    ]));
    // Grouped grammar predating this rename: `cortex ingest agent-command
    // ingest-spool`. This is the one already deployed on live hosts (e.g.
    // devhost) and the only legacy shape worth tolerating here — the even
    // older bare `cortex agent-command ingest-spool` (no `ingest` prefix) is
    // unreachable: the CLI's top-level parser rejects it outright (see
    // `src/surfaces.rs`'s `MovedIntoGroupedDomain` entry), so no process can
    // ever actually invoke it for this guard to need to catch.
    assert!(is_agent_command_ingest_spool_invocation(&[
        "cortex".to_string(),
        "ingest".to_string(),
        "agent-command".to_string(),
        "ingest-spool".to_string(),
    ]));
    assert!(!is_agent_command_ingest_spool_invocation(&[
        "sh".to_string(),
        "-c".to_string(),
        "cortex ingest shell agent index".to_string(),
    ]));
    assert!(!is_agent_command_ingest_spool_invocation(&[
        "bash".to_string(),
        "-c".to_string(),
        "agent-command ingest-spool".to_string(),
    ]));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib command_log_tests::agent_command_ingest_spool_guard_is_argv_scoped -- --nocapture`
Expected: FAIL — the canonical `["ingest", "shell", "agent", "index", ...]` shape isn't recognized yet.

- [ ] **Step 3: Add the new shape to `is_agent_command_ingest_spool_invocation`**

Replace:

```rust
fn is_agent_command_ingest_spool_invocation(command_args: &[String]) -> bool {
    let Some(program) = command_args.first() else {
        return false;
    };
    let program_name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program);
    if program_name != "cortex" {
        return false;
    }
    let rest: Vec<&str> = command_args[1..].iter().map(String::as_str).collect();
    // New grouped grammar: `cortex ingest agent-command ingest-spool`.
    // Legacy pre-move grammar (`cortex agent-command ingest-spool`) is accepted
    // defensively so a lingering caller can never be self-ingested.
    matches!(
        rest.as_slice(),
        ["ingest", "agent-command", "ingest-spool", ..] | ["agent-command", "ingest-spool", ..]
    )
}
```

with:

```rust
fn is_agent_command_ingest_spool_invocation(command_args: &[String]) -> bool {
    let Some(program) = command_args.first() else {
        return false;
    };
    let program_name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program);
    if program_name != "cortex" {
        return false;
    }
    let rest: Vec<&str> = command_args[1..].iter().map(String::as_str).collect();
    // Canonical grammar: `cortex ingest shell agent index`. Grouped
    // pre-restructure grammar: `cortex ingest agent-command ingest-spool` —
    // the one immediately-prior grammar already deployed on live hosts (e.g.
    // devhost), accepted defensively so a lingering unregenerated
    // wrapper/timer can never be self-ingested. The even-older bare
    // `cortex agent-command ingest-spool` (no `ingest` prefix) is
    // deliberately NOT tolerated here — it's unreachable, since the CLI's
    // top-level parser rejects that form outright (see `src/surfaces.rs`).
    matches!(
        rest.as_slice(),
        ["ingest", "shell", "agent", "index", ..]
            | ["ingest", "agent-command", "ingest-spool", ..]
    )
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib command_log_tests::agent_command_ingest_spool_guard_is_argv_scoped -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/command_log.rs src/command_log_tests.rs
git commit -m "fix: recognize renamed shell agent index grammar in self-ingest guard"
```

---

### Task 8: Update docs/registry surfaces (`help.rs`, `surfaces.rs`, `color.rs`)

**Files:**
- Modify: `src/cli/help.rs` (four `usage`/`path` blocks)
- Modify: `src/surfaces.rs` (one registry line)
- Modify: `src/cli/color.rs` (one comment)
- Test: `src/cli/help_tests.rs`

**Interfaces:**
- Consumes: nothing new
- Produces: nothing new — pure documentation/registry accuracy.

- [ ] **Step 1: Update the `ingest` `CommandDoc`'s usage lines**

Find:

```rust
    CommandDoc {
        name: "ingest",
        summary: "Manual ingest and ingest-source management",
        usage: &[
            "cortex ingest shell index --path PATH [--shell zsh] [--json]",
            "cortex ingest shell atuin-index --path PATH [--json]",
            "cortex ingest agent-command ingest-spool --path PATH [--json]",
            "cortex ingest agent-command wrap --spool PATH -- COMMAND...",
            "cortex ingest inventory refresh|status [--json]",
            "cortex ingest file-tail list|status [--json]",
            "cortex ingest file-tail add --id ID --path PATH --tag TAG --host HOST [--facility FACILITY] [--severity SEVERITY] [--from-start] [--json]",
            "cortex ingest file-tail remove --id ID [--json]",
        ],
    },
```

Replace with:

```rust
    CommandDoc {
        name: "ingest",
        summary: "Manual ingest and ingest-source management",
        usage: &[
            "cortex ingest shell user index --path PATH [--shell zsh] [--json]",
            "cortex ingest shell user atuin-index --path PATH [--json]",
            "cortex ingest shell agent index --path PATH [--json] [--server URL] [--token TOKEN]",
            "cortex ingest shell agent wrap --spool PATH -- COMMAND...",
            "cortex ingest inventory refresh|status [--json]",
            "cortex ingest file-tail list|status [--json]",
            "cortex ingest file-tail add --id ID --path PATH --tag TAG --host HOST [--facility FACILITY] [--severity SEVERITY] [--from-start] [--json]",
            "cortex ingest file-tail remove --id ID [--json]",
        ],
    },
```

- [ ] **Step 2: Update the `setup` `CommandDoc`'s usage lines**

Find:

```rust
            "cortex setup agent-command install|remove|check [--json]",
```

Replace with:

```rust
            "cortex setup shell agent install|remove|check [--json]",
            "cortex setup shell completions install|remove|check [--json]",
```

- [ ] **Step 3: Replace the two `NestedCommandDoc`s**

Find:

```rust
    NestedCommandDoc {
        path: "ingest shell",
        summary: "Index shell history",
        usage: &[
            "cortex ingest shell index --path PATH [--shell zsh] [--json]",
            "cortex ingest shell atuin-index --path PATH [--json]",
        ],
    },
    NestedCommandDoc {
        path: "ingest agent-command",
        summary: "Ingest agent command spool files",
        usage: &[
            "cortex ingest agent-command ingest-spool --path PATH [--json]",
            "cortex ingest agent-command wrap --spool PATH -- COMMAND...",
        ],
    },
```

Replace with:

```rust
    NestedCommandDoc {
        path: "ingest shell user",
        summary: "Index shell history typed by a human",
        usage: &[
            "cortex ingest shell user index --path PATH [--shell zsh] [--json]",
            "cortex ingest shell user atuin-index --path PATH [--json]",
        ],
    },
    NestedCommandDoc {
        path: "ingest shell agent",
        summary: "Ingest AI-agent-issued shell commands",
        usage: &[
            "cortex ingest shell agent index --path PATH [--json] [--server URL] [--token TOKEN]",
            "cortex ingest shell agent wrap --spool PATH -- COMMAND...",
        ],
    },
```

- [ ] **Step 4: Update `surfaces.rs`'s registry line**

Find:

```rust
    cli!("agent-command", Ingest, MovedIntoGroupedDomain, Admin, replace: "ingest agent-command", reason: "agent command ingestion lives under ingest"),
```

Replace with:

```rust
    cli!("agent-command", Ingest, MovedIntoGroupedDomain, Admin, replace: "ingest shell agent", reason: "agent command ingestion lives under ingest shell agent"),
```

- [ ] **Step 5: Update the stale comment in `color.rs`**

Find:

```rust
/// so wrapped commands (`cortex agent-command wrap -- cmd --color`) are left
```

Replace with:

```rust
/// so wrapped commands (`cortex ingest shell agent wrap -- cmd --color`) are left
```

- [ ] **Step 6: Search for any remaining stale usage strings before running tests**

Run: `grep -rn '"cortex ingest agent-command\|"cortex setup agent-command\|ingest agent-command wrap`\|ingest agent-command ingest-spool' src/`
Expected: no matches outside of intentional back-compat/legacy-grammar comments already updated above. If any remain, update them the same way.

- [ ] **Step 7: Run `help_tests.rs` and the full test suite**

Run: `cargo test --lib cli::help_tests -- --nocapture`
Expected: PASS. Then run the full suite:

Run: `cargo build --lib --bin cortex 2>&1 | tail -40 && cargo test --lib 2>&1 | tail -80`
Expected: both PASS with zero errors. This is the checkpoint that closes out Phase 1 — every CLI/setup file has been updated consistently.

- [ ] **Step 8: Regenerate the CLI help snapshot if one exists**

Run: `grep -rn "insta::assert" src/cli/help_tests.rs src/cli/complete_tests.rs 2>/dev/null`
Expected: if this returns matches (snapshot tests), run `cargo insta review` or `INSTA_UPDATE=always cargo test --lib cli::help_tests cli::complete_tests` and commit the updated `.snap` files alongside this task's commit. If no matches, skip — the repo doesn't currently use `insta` snapshots for these tests, so this step is a no-op.

- [ ] **Step 9: Commit**

```bash
git add src/cli/help.rs src/surfaces.rs src/cli/color.rs
git commit -m "docs(cli): update help/registry text for shell agent rename"
```

---

### Task 9: Update `main_tests.rs` for the renamed grammar

**Files:**
- Modify: `src/main_tests.rs` (tests at approximately lines 192, 251–263, 279–305, 520, 924, 933, 945 — re-locate with `grep -n` before editing since line numbers will have shifted after Tasks 1–8)
- Test: this task *is* the test file

**Interfaces:**
- Consumes: `ShellCommand::Agent`, `ShellAgentCommand::Wrap`/`Index` from Task 1
- Produces: nothing new — closes out Phase 1's end-to-end `Mode::parse` coverage.

- [ ] **Step 1: Re-locate every stale reference**

Run: `grep -n "agent-command\|agent_command\|AgentCommand\|ingest-spool\|ingest_spool\|IngestSpool" src/main_tests.rs`
Expected: a list of line numbers to edit (this repo's test file has these clustered around a setup-namespace test, a `mode_parse_accepts_command_ingest_namespace` test, `mode_parse_preserves_wrapped_command_http_like_flags`, a setup `remove` test, and the http-flag-rejection message tests).

- [ ] **Step 2: Rename `mode_parse_accepts_agent_command_setup_namespace`**

Replace:

```rust
#[test]
fn mode_parse_accepts_agent_command_setup_namespace() {
    assert!(matches!(
        Mode::parse(vec![
            "setup".into(),
            "agent-command".into(),
            "install".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Setup(_)
    ));
}
```

with:

```rust
#[test]
fn mode_parse_accepts_shell_agent_setup_namespace() {
    assert!(matches!(
        Mode::parse(vec![
            "setup".into(),
            "shell".into(),
            "agent".into(),
            "install".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Setup(_)
    ));
}

#[test]
fn mode_parse_accepts_shell_completions_setup_namespace() {
    assert!(matches!(
        Mode::parse(vec![
            "setup".into(),
            "shell".into(),
            "completions".into(),
            "install".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Setup(_)
    ));
}

#[test]
fn mode_parse_rejects_unknown_setup_shell_subcommand() {
    let error = Mode::parse(vec!["setup".into(), "shell".into(), "bogus".into()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("agent|completions"), "got: {error}");
}
```

- [ ] **Step 3: Update `mode_parse_accepts_command_ingest_namespace`**

Find the two `"agent-command"` blocks inside this test:

```rust
    assert!(matches!(
        Mode::parse(vec![
            "ingest".into(),
            "agent-command".into(),
            "ingest-spool".into(),
            "--path".into(),
            "/tmp/spool.jsonl".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Cli(_)
    ));
    assert!(matches!(
        Mode::parse(vec![
            "ingest".into(),
            "agent-command".into(),
            "wrap".into(),
            "--spool".into(),
            "/tmp/spool.jsonl".into(),
            "--".into(),
            "true".into()
        ])
        .unwrap(),
        Mode::Cli(_)
    ));
```

Replace with (covering both the canonical and legacy grammar):

```rust
    assert!(matches!(
        Mode::parse(vec![
            "ingest".into(),
            "shell".into(),
            "agent".into(),
            "index".into(),
            "--path".into(),
            "/tmp/spool.jsonl".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Cli(_)
    ));
    assert!(matches!(
        Mode::parse(vec![
            "ingest".into(),
            "shell".into(),
            "agent".into(),
            "wrap".into(),
            "--spool".into(),
            "/tmp/spool.jsonl".into(),
            "--".into(),
            "true".into()
        ])
        .unwrap(),
        Mode::Cli(_)
    ));
    assert!(matches!(
        Mode::parse(vec![
            "ingest".into(),
            "agent-command".into(),
            "ingest-spool".into(),
            "--path".into(),
            "/tmp/spool.jsonl".into(),
            "--json".into()
        ])
        .unwrap(),
        Mode::Cli(_)
    ));
```

- [ ] **Step 4: Update `mode_parse_preserves_wrapped_command_http_like_flags`**

Replace the invocation:

```rust
    let mode = Mode::parse(vec![
        "ingest".into(),
        "agent-command".into(),
        "wrap".into(),
        "--spool".into(),
        "/tmp/spool.jsonl".into(),
        "--".into(),
        "curl".into(),
        "--http".into(),
        "--server".into(),
        "https://example.test".into(),
        "--token=literal".into(),
    ])
    .unwrap();

    let Mode::Cli(invocation) = mode else {
        panic!("expected CLI mode");
    };
    assert_eq!(invocation.flags, cli::GlobalFlags::default());
    let cli::CliCommand::Ingest(cli::IngestCommand::AgentCommand(cli::AgentCommandCommand::Wrap(
        args,
    ))) = invocation.command
    else {
        panic!("expected agent-command wrap");
    };
```

with:

```rust
    let mode = Mode::parse(vec![
        "ingest".into(),
        "shell".into(),
        "agent".into(),
        "wrap".into(),
        "--spool".into(),
        "/tmp/spool.jsonl".into(),
        "--".into(),
        "curl".into(),
        "--http".into(),
        "--server".into(),
        "https://example.test".into(),
        "--token=literal".into(),
    ])
    .unwrap();

    let Mode::Cli(invocation) = mode else {
        panic!("expected CLI mode");
    };
    assert_eq!(invocation.flags, cli::GlobalFlags::default());
    let cli::CliCommand::Ingest(cli::IngestCommand::Shell(cli::ShellCommand::Agent(
        cli::ShellAgentCommand::Wrap(args),
    ))) = invocation.command
    else {
        panic!("expected shell agent wrap");
    };
```

(leave the rest of the test — the `assert_eq!(args.command, vec![...])` assertions — unchanged, since the wrapped command's own args are unaffected by this rename.)

- [ ] **Step 5: Update the `setup agent-command remove` test around line 520**

Find:

```rust
            vec!["setup", "agent-command", "remove", "--json"],
            "agent-command remove",
```

Replace with:

```rust
            vec!["setup", "shell", "agent", "remove", "--json"],
            "shell agent remove",
```

- [ ] **Step 6: Update the http-flag-rejection message tests around lines 924/933/945**

Run: `sed -n '900,950p' src/main_tests.rs` first to see the exact surrounding test bodies (these assert on the literal error strings changed in Task 5's Step 1–2), then update:
- Any `"agent-command"` literal in a constructed `Mode::parse` argv → `"shell"`, `"agent"` (two separate elements, matching the new nesting)
- The assertion `"`ingest agent-command wrap` (wrapper command)"` → `` "`ingest shell agent wrap` (wrapper command)"``
- The assertion `"local agent commands"` → `"local shell commands"` (matching Task 5 Step 2's renamed error message)

- [ ] **Step 7: Run the full test suite**

Run: `cargo test --lib 2>&1 | tail -80`
Expected: PASS, zero failures. This is Phase 1's final checkpoint.

- [ ] **Step 8: Commit**

```bash
git add src/main_tests.rs
git commit -m "test: update main_tests.rs for shell agent CLI rename"
```

---

## Phase 2 — `cortex setup shell completions`

### Task 10: Add the `setup shell completions` module

**This does not touch or replace the existing `cortex completions <shell>` command** (`src/cli/completions.rs`, which prints the zsh completion script to stdout for manual sourcing — untouched by this whole plan). This task only adds automated *installation* of that same script to disk, reusing its content, as a third `setup shell` child alongside `agent` — i.e. `cortex setup shell completions install|remove|check`, parallel to `cortex setup shell agent install|remove|check` from Task 6. Two different commands, same script content, both nested under `shell`.

**Engineering review flagged this as the most separable unit in the whole plan** — both the architecture and simplicity reviews independently identified it as unrelated to the CLI rename (Phases 1) and to both explicit secondary asks (forwarding, Phase 3; stale-timer detection, Phase 4). The only coupling is that Task 5/6 add `ShellSetupCommand::Completions` to the same nested-enum sweep as `ShellSetupCommand::Agent`. **If landing the rename/forwarding/detection quickly matters more than landing completions in the same PR, this task (plus the `Completions` arm in Task 5's `ShellSetupCommand`/`SetupCommandKind` and Task 6's `ShellCompletionsAction` declaration) can be deferred to a follow-up PR with zero risk to the other three phases** — just skip this task, drop the `Completions` variant/arm from Tasks 5–6, and re-add both later. Nothing elsewhere in this plan reads `ShellCompletionsAction` or calls `run_shell_completions_setup`.

**Files:**
- Create: `src/setup/shell_completions.rs`
- Create: `src/setup/shell_completions_tests.rs`
- Modify: `src/setup.rs` (module registration + re-export, already declared `ShellCompletionsAction` in Task 6)

**Interfaces:**
- Consumes: `cortex::cli::completions::zsh_completion_script()` — **note:** this is currently `pub(crate)` inside the CLI binary crate (`src/cli/completions.rs`), not the library crate (`cortex::...`). Since `src/setup.rs` lives in the **library** crate and `src/cli/completions.rs` lives in the **binary** crate, the library cannot import it directly. This task copies the same `include_str!("../../cli/completions/_cortex.zsh")` pattern independently in the library crate rather than trying to share code across the crate boundary — see Step 2.
- Produces: `cortex::setup::ShellCompletionsAction` (already declared in Task 6); `cortex::setup::run_shell_completions_setup(ShellCompletionsAction) -> io::Result<SetupReport>` — consumed by Task 5's `main.rs` dispatch (already wired).

- [ ] **Step 1: Confirm the completion script's location relative to the library crate**

Run: `find src -iname "_cortex.zsh"`
Expected: `src/cli/completions/_cortex.zsh`. Since this file lives under `src/cli/` (binary crate) and `src/setup/` is under the library crate root (`src/`), Step 2 below uses a relative `include_str!("../cli/completions/_cortex.zsh")` from `src/setup/shell_completions.rs` — verify this resolves by running `ls src/cli/completions/_cortex.zsh` from the repo root before proceeding (path is relative to the source file doing the `include_str!`, i.e. `src/setup/`).

- [ ] **Step 2: Write the failing test first**

Create `src/setup/shell_completions_tests.rs`:

```rust
use super::*;
use std::fs;

fn temp_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[tokio::test]
async fn install_writes_completion_script() {
    let home = temp_home();
    let path = shell_completions_install_path(home.path());
    let report = run_shell_completions_setup_at(ShellCompletionsAction::Install, home.path())
        .await
        .unwrap();
    assert_eq!(report.status, SetupStatus::Ok);
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("#compdef cortex"));
}

#[tokio::test]
async fn check_reports_warn_when_missing() {
    let home = temp_home();
    let report = run_shell_completions_setup_at(ShellCompletionsAction::Check, home.path())
        .await
        .unwrap();
    assert_ne!(report.status, SetupStatus::Ok);
}

#[tokio::test]
async fn check_reports_ok_after_install() {
    let home = temp_home();
    run_shell_completions_setup_at(ShellCompletionsAction::Install, home.path())
        .await
        .unwrap();
    let report = run_shell_completions_setup_at(ShellCompletionsAction::Check, home.path())
        .await
        .unwrap();
    assert_eq!(report.status, SetupStatus::Ok);
}

#[tokio::test]
async fn remove_deletes_completion_script() {
    let home = temp_home();
    run_shell_completions_setup_at(ShellCompletionsAction::Install, home.path())
        .await
        .unwrap();
    let path = shell_completions_install_path(home.path());
    assert!(path.exists());
    run_shell_completions_setup_at(ShellCompletionsAction::Remove, home.path())
        .await
        .unwrap();
    assert!(!path.exists());
}
```

(This test file calls a `run_shell_completions_setup_at(action, user_home)` test-seam function and a `shell_completions_install_path(user_home)` helper that Step 3 defines — mirroring how other `setup/*_tests.rs` files in this repo inject a temp home directory rather than touching the real `$HOME`. Check `src/setup/agent_command_tests.rs`'s existing pattern with `grep -n "fn temp_home\|_at(" src/setup/shell_agent_tests.rs` for the exact idiom this repo already uses, and match it — if that file uses a differently-named seam function, e.g. `run_agent_command_setup_at`, use that same naming convention here for consistency.)

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib setup::shell_completions_tests -- --nocapture`
Expected: FAIL to compile — `src/setup/shell_completions.rs` doesn't exist yet.

- [ ] **Step 4: Create `src/setup/shell_completions.rs`**

```rust
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{
    PhaseTimer, SetupPhase, SetupReport, SetupStatus, host_local_report_input, setup_report,
    write_executable_file,
};

const ZSH_COMPLETION_SCRIPT: &str = include_str!("../cli/completions/_cortex.zsh");

pub async fn run_shell_completions_setup(
    action: super::ShellCompletionsAction,
) -> io::Result<SetupReport> {
    let user_home = super::user_home_dir()?;
    run_shell_completions_setup_at(action, &user_home).await
}

async fn run_shell_completions_setup_at(
    action: super::ShellCompletionsAction,
    user_home: &Path,
) -> io::Result<SetupReport> {
    let started = Instant::now();
    let home = super::cortex_home_dir()?;
    let env_path = home.join(".env");
    let compose_dir = home.join("compose");
    let data_dir = home.join("data");
    let install_path = shell_completions_install_path(user_home);
    let mut phases = Vec::new();

    match action {
        super::ShellCompletionsAction::Install => {
            phases.push(install_shell_completions(&install_path)?);
            phases.push(shell_completions_fpath_hint_phase(user_home));
        }
        super::ShellCompletionsAction::Remove => {
            phases.push(remove_shell_completions(&install_path)?);
        }
        super::ShellCompletionsAction::Check => {
            phases.push(check_shell_completions_content_phase(&install_path));
            phases.push(shell_completions_fpath_hint_phase(user_home));
        }
    }

    let elapsed_ms = started.elapsed().as_millis();
    Ok(setup_report(
        host_local_report_input(action.as_str(), elapsed_ms, home, env_path, compose_dir, data_dir),
        phases,
    ))
}

fn shell_completions_install_path(user_home: &Path) -> PathBuf {
    user_home
        .join(".local/share/cortex/completions/_cortex")
}

fn install_shell_completions(install_path: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("shell-completions-files");
    if let Some(parent) = install_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_executable_file(install_path, ZSH_COMPLETION_SCRIPT)?;
    Ok(timer.finish(
        SetupStatus::Ok,
        format!("wrote {}", install_path.display()),
    ))
}

fn remove_shell_completions(install_path: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("shell-completions-wrapper");
    match std::fs::remove_file(install_path) {
        Ok(()) => Ok(timer.finish(
            SetupStatus::Ok,
            format!("removed {}", install_path.display()),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(timer.finish(
            SetupStatus::Ok,
            format!("{} already absent", install_path.display()),
        )),
        Err(error) => Err(error),
    }
}

fn check_shell_completions_content_phase(install_path: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("shell-completions-content");
    match std::fs::read_to_string(install_path) {
        Ok(current) if current == ZSH_COMPLETION_SCRIPT => {
            timer.finish(SetupStatus::Ok, "completion script matches generated content")
        }
        Ok(_) => timer.finish(
            SetupStatus::Error,
            format!(
                "{} does not match generated completion script; run cortex setup shell completions install",
                install_path.display()
            ),
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => timer.finish(
            SetupStatus::Warn,
            format!(
                "missing {}; run cortex setup shell completions install",
                install_path.display()
            ),
        ),
        Err(error) => timer.finish(SetupStatus::Error, error.to_string()),
    }
}

/// Read-only check: does `~/.zshrc` appear to add the completions directory
/// to `fpath`? `~/.zshrc` is chezmoi-managed on Jacob's hosts (see the
/// homelab CLAUDE.md's chezmoi rules) — this function never writes to it,
/// only warns with the exact line the operator should add themselves.
fn shell_completions_fpath_hint_phase(user_home: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("shell-completions-fpath");
    let zshrc = user_home.join(".zshrc");
    let expected_dir = user_home.join(".local/share/cortex/completions");
    let expected_dir = expected_dir.display().to_string();
    match std::fs::read_to_string(&zshrc) {
        Ok(content) if content.contains(&expected_dir) => {
            timer.finish(SetupStatus::Ok, format!("{} already sourced from ~/.zshrc", expected_dir))
        }
        Ok(_) => timer.finish(
            SetupStatus::Warn,
            format!(
                "add `fpath+=({expected_dir}); autoload -Uz compinit && compinit` to ~/.zshrc (chezmoi-managed; not edited automatically)"
            ),
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => timer.finish(
            SetupStatus::Warn,
            format!(
                "~/.zshrc not found; add `fpath+=({expected_dir}); autoload -Uz compinit && compinit` to your zsh init"
            ),
        ),
        Err(error) => timer.finish(SetupStatus::Warn, error.to_string()),
    }
}

#[cfg(test)]
#[path = "shell_completions_tests.rs"]
mod tests;
```

- [ ] **Step 5: Register the module in `src/setup.rs`**

Add `mod shell_completions;` alongside the other `mod` declarations, and `pub use shell_completions::run_shell_completions_setup;` alongside the other `pub use` lines (both already partially wired in Task 6 for the `ShellCompletionsAction` type — this step adds the module and function re-export).

- [ ] **Step 6: Run the tests**

Run: `cargo test --lib setup::shell_completions_tests -- --nocapture`
Expected: PASS

- [ ] **Step 7: Update `help.rs`'s `setup` doc entry if not already covered**

Task 8, Step 2 already added the `"cortex setup shell completions install|remove|check [--json]"` line — verify with `grep -n "shell completions" src/cli/help.rs` that it's present; if this task is executed independently of Phase 1, add it now using the exact text from Task 8 Step 2.

- [ ] **Step 8: Commit**

```bash
git add src/setup.rs src/setup/shell_completions.rs src/setup/shell_completions_tests.rs
git commit -m "feat(setup): add setup shell completions install/remove/check"
```

---

## Phase 3 — Forward agent-command spool to a remote Cortex

### Task 11: Extract `parse_agent_command_spool_lines` and `import_agent_command_records` in `command_log.rs`

**Files:**
- Modify: `src/command_log.rs` (the `import_agent_command_spool` function)
- Test: `src/command_log_tests.rs`

**Interfaces:**
- Consumes: `AgentCommandSpoolRecord`, `CommandLogImportResult` (existing types)
- Produces: `parse_agent_command_spool_lines(reader: impl BufRead) -> ParsedAgentCommandSpool` (private helper); `pub fn import_agent_command_records(pool: &db::DbPool, records: &[AgentCommandSpoolRecord], forwarded_from_peer: Option<&str>) -> Result<CommandLogImportResult>` (third parameter added per engineering review — see Step 3) — consumed by Task 13 (server handler, which passes the real `ConnectInfo` peer IP) and this task's own rewritten `import_agent_command_spool` (which passes `None`).

- [ ] **Step 1: Write the failing test first**

Add to `src/command_log_tests.rs` (find the existing `imports_agent_spool_as_agent_command_rows` test for the fixture-building pattern to copy):

```rust
#[test]
fn import_agent_command_records_dedupes_against_existing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(&dir);
    let record = AgentCommandSpoolRecord {
        started_at: "2026-07-06T00:00:00Z".to_string(),
        finished_at: "2026-07-06T00:00:01Z".to_string(),
        duration_ms: 1000,
        exit_status: Some(0),
        command: "echo hi".to_string(),
        cwd: None,
        agent: "claude-code".to_string(),
        command_surface: None,
        hostname: "testhost".to_string(),
        user: None,
        pid: 1234,
        session_id: None,
        schema_version: 1,
        content_scrubbed: true,
    };

    let first = import_agent_command_records(&pool, &[record.clone()], None).unwrap();
    assert_eq!(first.imported, 1);
    assert_eq!(first.skipped_duplicates, 0);

    let second = import_agent_command_records(&pool, &[record], None).unwrap();
    assert_eq!(second.imported, 0);
    assert_eq!(second.skipped_duplicates, 1);
}
```

(If this test file doesn't already have a `test_pool(&dir)` helper, use whatever helper `imports_agent_spool_as_agent_command_rows` already calls to build a `db::DbPool` — check with `grep -n "fn test_pool\|DbPool::" src/command_log_tests.rs` and match that exact helper name/signature.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib command_log_tests::import_agent_command_records_dedupes_against_existing_rows -- --nocapture`
Expected: FAIL to compile — `import_agent_command_records` doesn't exist yet.

- [ ] **Step 3: Extract the parse loop and dedupe/insert loop out of `import_agent_command_spool`**

Replace:

```rust
pub fn import_agent_command_spool(
    pool: &db::DbPool,
    path: &Path,
) -> Result<CommandLogImportResult> {
    validate_spool_path_for_read(path)?;
    let mut file = open_spool_for_update(path)?;
    lock_file_exclusive(&file, path)?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("seek agent command spool {}", path.display()))?;
    let reader = BufReader::new(&mut file);
    let mut result = CommandLogImportResult::default();
    let mut batch = Vec::new();

    for line in reader.lines() {
        result.scanned += 1;
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                tracing::warn!(
                    line = result.scanned,
                    error_kind = "io",
                    error = %e,
                    "failed to read line from agent command spool"
                );
                result.errors += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            result.skipped += 1;
            continue;
        }
        match serde_json::from_str::<AgentCommandSpoolRecord>(&line) {
            Ok(record) => {
                let entry = agent_record_to_entry(&record);
                if entry_exists(pool, &entry)? {
                    result.skipped_duplicates += 1;
                } else {
                    batch.push(entry);
                }
            }
            Err(e) => {
                tracing::warn!(
                    line = result.scanned,
                    error_kind = "json",
                    error = %e,
                    line_preview = %truncate_utf8(&line, 80),
                    "failed to parse agent command spool record"
                );
                result.errors += 1;
            }
        }
    }

    if !batch.is_empty() {
        result.imported = db::insert_logs_batch(pool, &batch)?;
    }
    file.set_len(0)
        .with_context(|| format!("truncate agent command spool {}", path.display()))?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("rewind agent command spool {}", path.display()))?;
    Ok(result)
}
```

with:

```rust
struct ParsedAgentCommandSpool {
    records: Vec<AgentCommandSpoolRecord>,
    scanned: usize,
    skipped: usize,
    errors: usize,
}

fn parse_agent_command_spool_lines(reader: impl BufRead) -> ParsedAgentCommandSpool {
    let mut parsed = ParsedAgentCommandSpool {
        records: Vec::new(),
        scanned: 0,
        skipped: 0,
        errors: 0,
    };
    for line in reader.lines() {
        parsed.scanned += 1;
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                tracing::warn!(
                    line = parsed.scanned,
                    error_kind = "io",
                    error = %e,
                    "failed to read line from agent command spool"
                );
                parsed.errors += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            parsed.skipped += 1;
            continue;
        }
        match serde_json::from_str::<AgentCommandSpoolRecord>(&line) {
            Ok(record) => parsed.records.push(record),
            Err(e) => {
                tracing::warn!(
                    line = parsed.scanned,
                    error_kind = "json",
                    error = %e,
                    line_preview = %truncate_utf8(&line, 80),
                    "failed to parse agent command spool record"
                );
                parsed.errors += 1;
            }
        }
    }
    parsed
}

/// Dedupes `records` against existing rows and inserts the remainder.
/// Shared by the local file-based import below and the server-side handler
/// in `agent_command_ingest.rs` that receives a forwarded batch over HTTP.
///
/// `forwarded_from_peer`: **engineering-review addition.** When `Some`, every
/// inserted row's `metadata_json` gets a `forwarded_from_peer_ip` field set
/// to this value. The rest of each record (`hostname`, `agent`,
/// `session_id`, which feed `source_ip`/`app_name`/`ai_tool`) remains fully
/// client-claimed and unverified — same as the pre-existing local-only
/// behavior — but recording the actual verified TCP peer alongside it means
/// a forged `hostname`/`agent` claim can be cross-referenced against which
/// token/peer really sent it, which local-only ingest never needed but the
/// network-reachable forwarding path (Task 13) does. Local callers
/// (`import_agent_command_spool` below) pass `None` — there's no remote peer
/// to record for a locally-read spool file.
pub fn import_agent_command_records(
    pool: &db::DbPool,
    records: &[AgentCommandSpoolRecord],
    forwarded_from_peer: Option<&str>,
) -> Result<CommandLogImportResult> {
    let mut result = CommandLogImportResult::default();
    let mut batch = Vec::new();
    for record in records {
        let mut entry = agent_record_to_entry(record);
        if let Some(peer_ip) = forwarded_from_peer {
            annotate_forwarded_peer(&mut entry, peer_ip);
        }
        if entry_exists(pool, &entry)? {
            result.skipped_duplicates += 1;
        } else {
            batch.push(entry);
        }
    }
    if !batch.is_empty() {
        result.imported = db::insert_logs_batch(pool, &batch)?;
    }
    Ok(result)
}

/// Merges a `forwarded_from_peer_ip` field into an already-built entry's
/// `metadata_json`, preserving whatever `agent_record_to_entry` already put
/// there. `metadata_json` is always `Some` coming out of
/// `agent_record_to_entry` (it always calls `bounded_metadata_json`), so the
/// `unwrap_or_default` here is defensive only.
fn annotate_forwarded_peer(entry: &mut LogBatchEntry, peer_ip: &str) {
    let mut value: serde_json::Value = entry
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let serde_json::Value::Object(map) = &mut value {
        map.insert(
            "forwarded_from_peer_ip".to_string(),
            serde_json::Value::String(peer_ip.to_string()),
        );
    }
    entry.metadata_json = Some(bounded_metadata_json(value));
}

pub fn import_agent_command_spool(
    pool: &db::DbPool,
    path: &Path,
) -> Result<CommandLogImportResult> {
    validate_spool_path_for_read(path)?;
    let mut file = open_spool_for_update(path)?;
    lock_file_exclusive(&file, path)?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("seek agent command spool {}", path.display()))?;
    let parsed = parse_agent_command_spool_lines(BufReader::new(&mut file));
    let mut result = import_agent_command_records(pool, &parsed.records, None)?;
    result.scanned = parsed.scanned;
    result.skipped += parsed.skipped;
    result.errors += parsed.errors;
    file.set_len(0)
        .with_context(|| format!("truncate agent command spool {}", path.display()))?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("rewind agent command spool {}", path.display()))?;
    Ok(result)
}
```

(`bounded_metadata_json` and `LogBatchEntry` are already imported/used elsewhere in this file per `agent_record_to_entry`'s existing body — no new imports needed.)

- [ ] **Step 4: Run the new test and the pre-existing spool-import tests**

Run: `cargo test --lib command_log_tests -- --nocapture`
Expected: PASS — both the new `import_agent_command_records_dedupes_against_existing_rows` test and the pre-existing `imports_agent_spool_as_agent_command_rows` test (which exercises `import_agent_command_spool` end-to-end and must still behave identically after this refactor).

- [ ] **Step 4b: Add a test proving the peer-IP annotation only appears when forwarding**

```rust
#[test]
fn import_agent_command_records_annotates_forwarded_peer_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(&dir);
    let record = AgentCommandSpoolRecord {
        started_at: "2026-07-06T00:00:00Z".to_string(),
        finished_at: "2026-07-06T00:00:01Z".to_string(),
        duration_ms: 1000,
        exit_status: Some(0),
        command: "echo hi".to_string(),
        cwd: None,
        agent: "claude-code".to_string(),
        command_surface: None,
        hostname: "testhost".to_string(),
        user: None,
        pid: 1234,
        session_id: None,
        schema_version: 1,
        content_scrubbed: true,
    };

    let result =
        import_agent_command_records(&pool, &[record], Some("203.0.113.7")).unwrap();
    assert_eq!(result.imported, 1);

    // Query the inserted row directly to prove the peer IP actually landed
    // in metadata_json, rather than just asserting the call didn't panic.
    let conn = pool.get().unwrap();
    let metadata_json: String = conn
        .query_row(
            "SELECT metadata_json FROM logs WHERE message = ?1",
            ["echo hi"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        metadata_json.contains("203.0.113.7"),
        "expected forwarded_from_peer_ip in metadata_json, got: {metadata_json}"
    );

    // A second record with no forwarding peer must NOT gain the field.
    let local_record = AgentCommandSpoolRecord {
        started_at: "2026-07-06T00:00:02Z".to_string(),
        finished_at: "2026-07-06T00:00:03Z".to_string(),
        duration_ms: 1000,
        exit_status: Some(0),
        command: "echo local".to_string(),
        cwd: None,
        agent: "claude-code".to_string(),
        command_surface: None,
        hostname: "testhost".to_string(),
        user: None,
        pid: 1234,
        session_id: None,
        schema_version: 1,
        content_scrubbed: true,
    };
    import_agent_command_records(&pool, &[local_record], None).unwrap();
    let local_metadata_json: String = conn
        .query_row(
            "SELECT metadata_json FROM logs WHERE message = ?1",
            ["echo local"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!local_metadata_json.contains("forwarded_from_peer_ip"));
}
```

- [ ] **Step 5: Commit**

```bash
git add src/command_log.rs src/command_log_tests.rs
git commit -m "refactor(command_log): extract shared parse/import helpers for spool forwarding"
```

---

### Task 12: Add `forward_agent_command_spool` (client-side POST-then-truncate)

**Files:**
- Modify: `src/command_log.rs` (replace the Phase 1 stub added in Task 3, Step 2)
- Test: `src/command_log_tests.rs`

**Interfaces:**
- Consumes: `parse_agent_command_spool_lines`, `AgentCommandSpoolRecord`, `CommandLogImportResult` from Task 11
- Produces: `pub async fn forward_agent_command_spool(path: &Path, target: &str, token: Option<&str>) -> Result<CommandLogImportResult>` — consumed by Task 3's `run_shell_agent_index_remote` (already wired against the stub; this task replaces the stub body).

- [ ] **Step 1: Write the failing test first**

This test needs an HTTP server to POST to. Add to `src/command_log_tests.rs`:

```rust
#[tokio::test]
async fn forward_agent_command_spool_posts_and_truncates_on_success() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let spool_path = dir.path().join("agent-command.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&spool_path)
        .unwrap();
    writeln!(
        file,
        r#"{{"started_at":"2026-07-06T00:00:00Z","finished_at":"2026-07-06T00:00:01Z","duration_ms":1000,"exit_status":0,"command":"echo hi","cwd":null,"agent":"claude-code","command_surface":null,"hostname":"testhost","user":null,"pid":1234,"session_id":null,"schema_version":1,"content_scrubbed":true}}"#
    )
    .unwrap();
    drop(file);

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/agent-commands")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"scanned":0,"imported":1,"skipped":0,"skipped_duplicates":0,"errors":0}"#)
        .create_async()
        .await;

    let result = forward_agent_command_spool(&spool_path, &server.url(), Some("secret"))
        .await
        .unwrap();

    mock.assert_async().await;
    assert_eq!(result.imported, 1);
    let remaining = std::fs::metadata(&spool_path).unwrap();
    assert_eq!(remaining.len(), 0, "spool must be truncated after a successful forward");
}

#[tokio::test]
async fn forward_agent_command_spool_keeps_file_on_http_failure() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let spool_path = dir.path().join("agent-command.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&spool_path)
        .unwrap();
    writeln!(
        file,
        r#"{{"started_at":"2026-07-06T00:00:00Z","finished_at":"2026-07-06T00:00:01Z","duration_ms":1000,"exit_status":0,"command":"echo hi","cwd":null,"agent":"claude-code","command_surface":null,"hostname":"testhost","user":null,"pid":1234,"session_id":null,"schema_version":1,"content_scrubbed":true}}"#
    )
    .unwrap();
    drop(file);

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/agent-commands")
        .with_status(503)
        .create_async()
        .await;

    let error = forward_agent_command_spool(&spool_path, &server.url(), None)
        .await
        .unwrap_err();

    mock.assert_async().await;
    assert!(error.to_string().contains("503"), "got: {error}");
    let remaining = std::fs::metadata(&spool_path).unwrap();
    assert!(remaining.len() > 0, "spool must survive a failed forward");
}
```

- [ ] **Step 2: Confirm the `mockito` dev-dependency exists (or add it)**

Run: `grep -n "^mockito" Cargo.toml`
Expected: if present, skip to Step 3. If absent, add under `[dev-dependencies]`:

```toml
mockito = "1"
```

Then run `cargo build --lib --tests 2>&1 | tail -20` to confirm the new dev-dependency resolves.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib command_log_tests::forward_agent_command_spool -- --nocapture`
Expected: FAIL — the Task 3 stub always returns `Err("forwarding not yet implemented")`.

- [ ] **Step 4: Replace the stub with the real implementation**

Replace the stub added in Task 3, Step 2:

```rust
/// Forwards a local agent-command spool to a remote Cortex instead of
/// writing to a local `DbPool`. Real implementation lands in Phase 3 (Task
/// 8) — this stub exists so Phase 1's CLI rename compiles independently.
pub async fn forward_agent_command_spool(
    _path: &Path,
    _target: &str,
    _token: Option<&str>,
) -> Result<CommandLogImportResult> {
    anyhow::bail!("forwarding not yet implemented")
}
```

with:

```rust
/// Reads and truncates the on-disk agent-command spool the same way
/// [`import_agent_command_spool`] does, but POSTs the parsed records to a
/// remote Cortex's `/v1/agent-commands` endpoint instead of writing to a
/// local `DbPool`. Truncates only after the remote POST succeeds, so a
/// network failure leaves the spool intact for the next attempt — mirroring
/// the heartbeat agent's retry-safe POST-then-truncate pattern in
/// `heartbeat_agent.rs`.
///
/// **Engineering-review addition:** the client has an explicit 30s request
/// timeout (`heartbeat_agent.rs`'s own `reqwest::Client::new()` has none
/// either, but this plan isn't the place to fix that pre-existing gap —
/// however, review flagged that a brand-new client shouldn't repeat the same
/// omission). Without this, a remote Cortex that's *hung* rather than down
/// would block the CLI invocation indefinitely with no feedback.
pub async fn forward_agent_command_spool(
    path: &Path,
    target: &str,
    token: Option<&str>,
) -> Result<CommandLogImportResult> {
    validate_spool_path_for_read(path)?;
    let mut file = open_spool_for_update(path)?;
    lock_file_exclusive(&file, path)?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("seek agent command spool {}", path.display()))?;
    let parsed = parse_agent_command_spool_lines(BufReader::new(&mut file));
    let mut result = CommandLogImportResult {
        scanned: parsed.scanned,
        skipped: parsed.skipped,
        errors: parsed.errors,
        ..Default::default()
    };

    if !parsed.records.is_empty() {
        let url = agent_command_forward_url(target)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("failed to build agent-command forwarding reqwest::Client")?;
        let mut request = client.post(url).json(&parsed.records);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .context("agent command forward POST failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("agent command forward POST returned {status}: {body}");
        }
        let remote: CommandLogImportResult = response
            .json()
            .await
            .context("agent command forward response was not valid JSON")?;
        result.imported = remote.imported;
        result.skipped_duplicates = remote.skipped_duplicates;
        result.errors += remote.errors;
    }

    file.set_len(0)
        .with_context(|| format!("truncate agent command spool {}", path.display()))?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("rewind agent command spool {}", path.display()))?;
    Ok(result)
}

fn agent_command_forward_url(target: &str) -> Result<String> {
    let trimmed = target.trim_end_matches('/');
    if trimmed.ends_with("/v1/agent-commands") {
        return Ok(trimmed.to_string());
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(format!("{trimmed}/v1/agent-commands"));
    }
    bail!("agent command forward target must start with http:// or https://");
}
```

(add `use anyhow::bail;` to this file's `use anyhow::{Context, Result};` line at the top if `bail!` isn't already imported — check with `grep -n "^use anyhow" src/command_log.rs`.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib command_log_tests::forward_agent_command_spool -- --nocapture`
Expected: PASS — both the success-and-truncate and failure-and-preserve tests.

- [ ] **Step 6: Commit**

```bash
git add src/command_log.rs Cargo.toml Cargo.lock
git commit -m "feat(command_log): forward agent-command spool to a remote Cortex"
```

---

### Task 13: Add the server-side `/v1/agent-commands` ingest endpoint

**Engineering-review changes applied to this task**: (1) a `MAX_RECORDS_PER_BATCH` cap is added — the 1 MiB body-size limit alone bounds bytes, not record count, and a dense batch of small `AgentCommandSpoolRecord`s could still pack thousands of records into 1 MiB, each triggering a synchronous per-record dedupe query; a batch that exceeds the cap is rejected outright rather than accepted and processed; (2) the handler now passes the real `ConnectInfo<SocketAddr>` peer IP into `import_agent_command_records`'s new `forwarded_from_peer` parameter (Task 11), so a forged `hostname`/`agent` claim in the payload can be cross-referenced against which token/peer actually sent it — this endpoint is the first place those client-claimed fields become network-reachable, so it's the right place to start recording the verified counterpart, even though the existing local-only behavior of trusting those fields for `source_ip`/`app_name`/`ai_tool` is unchanged (that's a pre-existing, out-of-scope pattern this plan doesn't fix); (3) a join-error/panic in the `spawn_blocking` task now returns a distinguishable error category to the caller instead of collapsing into the same generic `internal_error` as every other failure.

**Files:**
- Create: `src/agent_command_ingest.rs`
- Create: `src/agent_command_ingest_tests.rs`
- Modify: `src/lib.rs` (module declaration)
- Modify: `src/runtime.rs` (`RuntimeCore::agent_command_router()`, already referenced from Task 5's `main.rs`)

**Interfaces:**
- Consumes: `command_log::import_agent_command_records`, `command_log::AgentCommandSpoolRecord` from Task 11; `crate::mcp::AuthPolicy`; `crate::db::DbPool`
- Produces: `pub struct AgentCommandIngestState`; `pub fn router(state: AgentCommandIngestState) -> axum::Router`; `pub const MAX_RECORDS_PER_BATCH: usize`; `RuntimeCore::agent_command_router(&self) -> axum::Router` — consumed by Task 5's `main.rs` (`app.merge(runtime.agent_command_router())`, already wired there).

- [ ] **Step 1: Write the failing integration test first**

Create `src/agent_command_ingest_tests.rs`:

```rust
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use super::*;
use crate::db::test_support::test_pool;
use crate::mcp::AuthPolicy;

fn test_state(token: Option<&str>) -> AgentCommandIngestState {
    let pool = Arc::new(test_pool());
    AgentCommandIngestState::new(pool, token.map(str::to_string), AuthPolicy::StaticToken)
}

#[tokio::test]
async fn rejects_missing_bearer_token() {
    let app = router(test_state(Some("secret")));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agent-commands")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("[]"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepts_batch_with_valid_bearer_token() {
    let app = router(test_state(Some("secret")));
    let body = serde_json::to_string(&[serde_json::json!({
        "started_at": "2026-07-06T00:00:00Z",
        "finished_at": "2026-07-06T00:00:01Z",
        "duration_ms": 1000,
        "exit_status": 0,
        "command": "echo hi",
        "cwd": null,
        "agent": "claude-code",
        "command_surface": null,
        "hostname": "testhost",
        "user": null,
        "pid": 1234,
        "session_id": null,
        "schema_version": 1,
        "content_scrubbed": true
    })])
    .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agent-commands")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let result: crate::command_log::CommandLogImportResult =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result.imported, 1);
}

#[tokio::test]
async fn rejects_malformed_json_body() {
    let app = router(test_state(Some("secret")));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agent-commands")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from("not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_batch_over_max_records() {
    // engineering-review addition: a batch exceeding MAX_RECORDS_PER_BATCH
    // must be rejected outright, not accepted and processed — the 1 MiB body
    // cap alone bounds bytes, not record count.
    let app = router(test_state(Some("secret")));
    let one_record = serde_json::json!({
        "started_at": "2026-07-06T00:00:00Z",
        "finished_at": "2026-07-06T00:00:01Z",
        "duration_ms": 1,
        "exit_status": 0,
        "command": "x",
        "cwd": null,
        "agent": "claude-code",
        "command_surface": null,
        "hostname": "testhost",
        "user": null,
        "pid": 1,
        "session_id": null,
        "schema_version": 1,
        "content_scrubbed": true
    });
    let too_many: Vec<serde_json::Value> =
        std::iter::repeat(one_record).take(MAX_RECORDS_PER_BATCH + 1).collect();
    let body = serde_json::to_string(&too_many).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agent-commands")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer secret")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
```

(This test references `crate::db::test_support::test_pool()` — check with `grep -rn "pub fn test_pool\|mod test_support" src/db.rs src/db/*.rs 2>/dev/null` for this repo's actual existing shared test-pool helper and its real path/name, e.g. `src/heartbeat_tests.rs` almost certainly builds a `DbPool` the same way for its own handler tests — copy that exact helper invocation instead of guessing, since a wrong path here won't compile. Check `AuthPolicy::StaticToken` is the correct variant name via `grep -n "enum AuthPolicy" -A6 src/mcp.rs src/mcp/*.rs 2>/dev/null` and use whatever variant name actually exists for "bearer-token-required" mode — `heartbeat_tests.rs` will already exercise this same enum and is the fastest way to find the right variant.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib agent_command_ingest_tests -- --nocapture`
Expected: FAIL to compile — `src/agent_command_ingest.rs` doesn't exist yet.

- [ ] **Step 3: Create `src/agent_command_ingest.rs`**

```rust
//! Remote agent-command ingest (`POST /v1/agent-commands`) — receives a
//! batch of `AgentCommandSpoolRecord`s forwarded from a satellite host's
//! local spool (see `command_log::forward_agent_command_spool`) and inserts
//! them into this server's own log store, deduping the same way local
//! `cortex ingest shell agent index` does via
//! `command_log::import_agent_command_records`.
//!
//! Mounted on the shared HTTP listener (port 3100) next to MCP, OTLP, and
//! heartbeats. Auth mirrors heartbeats (`src/heartbeat.rs`): static
//! `CORTEX_TOKEN` bearer when configured, loopback-only otherwise.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::post,
};
use bytes::Bytes;
use lab_auth::middleware::{parse_bearer_token, tokens_equal};
use serde_json::json;
use tower_http::limit::RequestBodyLimitLayer;

use crate::command_log::{self, AgentCommandSpoolRecord};
use crate::db::DbPool;
use crate::mcp::AuthPolicy;

pub const AGENT_COMMAND_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Caps record *count*, independent of the byte-size limit above. A dense
/// batch of small `AgentCommandSpoolRecord`s (each roughly 150-400 bytes of
/// JSON) could still pack several thousand records into 1 MiB, and each
/// record triggers one synchronous dedupe query in
/// `command_log::import_agent_command_records` — engineering review flagged
/// this as the actual scaling risk, not the byte cap. 5,000 records is a
/// generous multiple of what a single drain cycle of one host's local spool
/// should ever accumulate between runs.
pub const MAX_RECORDS_PER_BATCH: usize = 5_000;

#[derive(Clone)]
pub struct AgentCommandIngestState {
    pool: Arc<DbPool>,
    api_token: Option<String>,
    auth_policy: AuthPolicy,
}

impl AgentCommandIngestState {
    pub fn new(pool: Arc<DbPool>, api_token: Option<String>, auth_policy: AuthPolicy) -> Self {
        Self {
            pool,
            api_token,
            auth_policy,
        }
    }
}

pub fn router(state: AgentCommandIngestState) -> Router {
    Router::new()
        .route("/v1/agent-commands", post(ingest_handler))
        .layer(RequestBodyLimitLayer::new(AGENT_COMMAND_BODY_LIMIT_BYTES))
        .with_state(state)
}

async fn ingest_handler(
    State(state): State<AgentCommandIngestState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_authorized(&state, &peer, &headers) {
        return unauthorized();
    }

    let records: Vec<AgentCommandSpoolRecord> = match serde_json::from_slice(&body) {
        Ok(records) => records,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_payload", "message": error.to_string()})),
            )
                .into_response();
        }
    };

    if records.len() > MAX_RECORDS_PER_BATCH {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "batch_too_large",
                "message": format!(
                    "batch has {} records, exceeds the {MAX_RECORDS_PER_BATCH}-record limit per request",
                    records.len()
                ),
            })),
        )
            .into_response();
    }

    let pool = Arc::clone(&state.pool);
    let peer_ip = peer.ip().to_string();
    let join_result = tokio::task::spawn_blocking(move || {
        command_log::import_agent_command_records(&pool, &records, Some(&peer_ip))
    })
    .await;

    match join_result {
        Ok(Ok(result)) => (StatusCode::OK, Json(result)).into_response(),
        Ok(Err(error)) => {
            tracing::error!(error = %error, "agent command forward ingest failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_error"})),
            )
                .into_response()
        }
        Err(join_error) => {
            // Distinguish "the blocking task panicked/was cancelled" from an
            // ordinary DB error — engineering review flagged the prior
            // version collapsing both into the same generic `internal_error`
            // with no way for a forwarding client to tell them apart.
            tracing::error!(error = %join_error, "agent command ingest task panicked or was cancelled");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "ingest_task_failed", "message": join_error.to_string()})),
            )
                .into_response()
        }
    }
}

fn is_authorized(state: &AgentCommandIngestState, peer: &SocketAddr, headers: &HeaderMap) -> bool {
    if matches!(state.auth_policy, AuthPolicy::LoopbackDev) {
        return peer.ip().is_loopback();
    }
    let Some(expected) = state.api_token.as_deref() else {
        return false;
    };
    let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    parse_bearer_token(auth).is_some_and(|token| tokens_equal(&token, expected))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

#[cfg(test)]
#[path = "agent_command_ingest_tests.rs"]
mod tests;
```

**Note on the test's `ConnectInfo` requirement:** `axum::extract::ConnectInfo<SocketAddr>` normally requires the router to be served via `into_make_service_with_connect_info::<SocketAddr>()`, which a bare `.oneshot()` call in a unit test does not provide automatically. Check how `src/heartbeat_tests.rs` (which has the identical `ConnectInfo` extractor on its handler) solves this in its own test suite — likely either by wrapping the test request through `Router::into_make_service_with_connect_info` or by injecting the extension manually via `.layer(axum::Extension(...))`/`request.extensions_mut().insert(ConnectInfo(addr))`. Copy that exact pattern into this task's test file rather than reinventing it, since `heartbeat_tests.rs` has already solved this problem for the same extractor shape.

- [ ] **Step 4: Register the module in `src/lib.rs`**

Find:

```rust
pub mod heartbeat;
pub mod heartbeat_agent;
```

Replace with:

```rust
pub mod agent_command_ingest;
pub mod heartbeat;
pub mod heartbeat_agent;
```

- [ ] **Step 5: Add `RuntimeCore::agent_command_router()` in `src/runtime.rs`**

Find:

```rust
    /// Build the heartbeat telemetry ingest router.
    pub fn heartbeat_router(&self) -> axum::Router {
        let state = HeartbeatState::new(
            Arc::clone(&self.pool),
            self.config.mcp.api_token.0.clone(),
            self.auth_policy.clone(),
        );
        crate::heartbeat::router(state)
    }
```

Add immediately after:

```rust

    /// Build the forwarded agent-command ingest router.
    pub fn agent_command_router(&self) -> axum::Router {
        let state = crate::agent_command_ingest::AgentCommandIngestState::new(
            Arc::clone(&self.pool),
            self.config.mcp.api_token.0.clone(),
            self.auth_policy.clone(),
        );
        crate::agent_command_ingest::router(state)
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test --lib agent_command_ingest_tests -- --nocapture`
Expected: PASS — all four tests (unauthorized, accepted-with-token, malformed-body, batch-too-large).

- [ ] **Step 6b: Add a test proving the merged app router actually boots with both `/v1/heartbeats` and `/v1/agent-commands`**

**Engineering-review addition.** `axum::Router::merge` panics at *runtime*, not compile time, on duplicate route registration — nothing in this plan otherwise proves `main.rs`'s `app.merge(runtime.agent_command_router())` (added right after the heartbeat merge) actually succeeds when combined with every other router mounted in `serve_mcp()` (MCP, OTLP, API, heartbeat, web_app). Task 13's own tests build a router directly from `AgentCommandIngestState`, bypassing that merge chain entirely.

Run: `find src -iname "runtime_tests.rs"` and `grep -n "fn.*router\|RuntimeCore::load" src/runtime_tests.rs 2>/dev/null` first to check whether a test already boots a merged/multi-router `RuntimeCore` app; if one exists, extend it to also merge `runtime.agent_command_router()` and assert a request to `/v1/agent-commands` doesn't panic and returns something other than 404. If no such test exists, add a minimal one to `src/runtime_tests.rs` (create it if absent, following this crate's existing `RuntimeCore` test-construction pattern — check how other `runtime.rs` tests, if any, build a `RuntimeCore` against a temp DB):

```rust
#[tokio::test]
async fn merged_app_serves_both_heartbeat_and_agent_command_routers_without_panicking() {
    // Build against a temp/in-memory RuntimeCore the same way this file's
    // other tests do (adjust to match whatever helper already exists here).
    let runtime = test_runtime_core().await;
    let app = axum::Router::new()
        .merge(runtime.heartbeat_router())
        .merge(runtime.agent_command_router());

    let heartbeat_response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/heartbeats")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(heartbeat_response.status(), axum::http::StatusCode::NOT_FOUND);

    let agent_command_response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/agent-commands")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from("[]"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(agent_command_response.status(), axum::http::StatusCode::NOT_FOUND);
}
```

(`test_runtime_core()` is illustrative — replace with whatever this crate's actual existing helper for constructing a test `RuntimeCore` is named; the load-bearing assertion is that `.merge()`-ing both routers together doesn't panic and both paths resolve to something other than 404, proving the two new/existing routes don't collide.)

Run: `cargo test --lib runtime_tests::merged_app_serves_both_heartbeat_and_agent_command_routers_without_panicking -- --nocapture`
Expected: PASS

- [ ] **Step 7: Run the full test suite and clippy**

Run: `cargo build --lib --bin cortex 2>&1 | tail -60 && cargo test --lib 2>&1 | tail -100 && RUSTC_WRAPPER='' cargo clippy --config 'build.rustc-wrapper=""' --all-targets -- -D warnings 2>&1 | tail -60`
Expected: all PASS. This is Phase 3's final checkpoint — the full local↔remote forwarding path is wired end-to-end and the merged HTTP app serves `/v1/agent-commands`.

- [ ] **Step 8: Commit**

```bash
git add src/agent_command_ingest.rs src/agent_command_ingest_tests.rs src/lib.rs src/runtime.rs src/runtime_tests.rs
git commit -m "feat: add /v1/agent-commands ingest endpoint for forwarded spool batches"
```

---

## Phase 4 — Lightweight stale-timer/stale-grammar detection

### Task 14: Add a stale agent-command-grammar scan to `cortex doctor`

**Engineering-review changes applied to this task** (see the eng-review notes at the end of this plan for full rationale): (1) the whole systemd scan now runs inside `tokio::task::spawn_blocking` — this codebase already has one prior incident (`ai_watcher_process_start_time()` in `src/app/watch_status.rs`) where a blocking `Command::output()` call was made directly inside an `async fn` and stalled a Tokio worker thread; this task's scan loops over *every* systemd `--user` unit (450 present on the reference host at plan-writing time) and would reproduce the same bug, worse, via fan-out; (2) stale-grammar detection is now anchored to the `ExecStart=` line specifically, using the same basename+argv-shape check `is_agent_command_ingest_spool_invocation` (Task 7) already uses, instead of a raw substring search over the whole unit file text (which could false-positive on a `Description=`/comment that merely mentions the old grammar and get an unrelated live unit disabled); (3) `--fix` now requires an additional `--yes` before it will actually run `disable --now`, matching this repo's own `cortex compose down` precedent of refusing destructive action without an explicit `--yes`; (4) the disable result is checked and reported, not discarded; (5) `systemctl --user cat` failures are logged instead of silently skipped, so an empty report is distinguishable from "systemd unreachable"; (6) unit names are pre-filtered by a cheap substring check before `cat`-ing each one, bounding the fan-out.

**Files:**
- Modify: `src/setup/doctor.rs`
- Modify: `src/setup/systemd.rs` (reuse `systemctl_user_state`, widen `run_systemctl_user` visibility — see Step 4)
- Test: `src/setup/doctor_tests.rs`

**Interfaces:**
- Consumes: `super::systemd::systemctl_user_state(command: &str, unit: &str) -> Option<String>` (already `pub(crate)`); `command_log`-style basename+argv-shape checking, mirroring `is_agent_command_ingest_spool_invocation` from Task 7 (not called directly — this task has its own copy scoped to `ExecStart=` text rather than live process argv, but uses the identical matching approach).
- Produces: a new `SetupPhase` appended to `cortex doctor`'s report, plus `--fix` and `--fix --yes` flags on the doctor CLI surface. Nothing downstream depends on new types from this task — it's a leaf feature.

- [ ] **Step 1: Read the current `doctor.rs` structure to match its existing phase style**

Run: `sed -n '1,90p' src/setup/doctor.rs`
Expected: shows the existing `run_setup_doctor()` function and its phase-collection pattern (mirrors `agent_command.rs`'s `PhaseTimer`/`SetupPhase`/`SetupStatus` idiom already documented above). Confirm the exact function signature and whether it already accepts a `fix: bool` parameter or needs one added — check `grep -n "pub async fn run_setup_doctor\|--fix" src/setup/doctor.rs src/main.rs` first, since `main.rs`'s `Doctor` mode currently only supports `[--json]` (per `help.rs`'s `"cortex doctor [--json]"` line) and will need new `--fix`/`--yes` flags threaded through if none exist elsewhere in this file for a different check.

- [ ] **Step 2: Write the failing tests first**

Add to `src/setup/doctor_tests.rs` (or create it if it doesn't exist — check with `ls src/setup/doctor_tests.rs`):

```rust
#[test]
fn stale_agent_command_grammar_detects_old_grammar_in_exec_start() {
    let unit_text = "\
[Unit]\nDescription=agent command drain\n\n[Service]\nExecStart=/usr/local/bin/cortex ingest agent-command ingest-spool --path /home/jmagar/.local/state/cortex/agent-command.jsonl\n";
    assert!(agent_command_unit_uses_stale_grammar(unit_text));
}

#[test]
fn stale_agent_command_grammar_accepts_current_grammar_in_exec_start() {
    let unit_text = "\
[Unit]\nDescription=agent command drain\n\n[Service]\nExecStart=/usr/local/bin/cortex ingest shell agent index --path /home/jmagar/.local/state/cortex/agent-command.jsonl\n";
    assert!(!agent_command_unit_uses_stale_grammar(unit_text));
}

#[test]
fn stale_agent_command_grammar_ignores_unrelated_unit_text() {
    let unit_text = "\
[Unit]\nDescription=some other timer\n\n[Service]\nExecStart=/usr/bin/true\n";
    assert!(!agent_command_unit_uses_stale_grammar(unit_text));
}

#[test]
fn stale_agent_command_grammar_ignores_mentions_outside_exec_start() {
    // A unit whose Description/comment merely *mentions* the old grammar
    // string must NOT be flagged — only the actual ExecStart= invocation
    // counts. This is the false-positive this task's ExecStart-anchored
    // check exists specifically to prevent.
    let unit_text = "\
[Unit]\nDescription=watches for agent-command ingest-spool usage in logs\n\n[Service]\nExecStart=/usr/bin/true\n";
    assert!(!agent_command_unit_uses_stale_grammar(unit_text));
}

#[test]
fn stale_agent_command_grammar_ignores_non_cortex_binary() {
    let unit_text = "\
[Unit]\nDescription=unrelated\n\n[Service]\nExecStart=/usr/bin/some-other-tool agent-command ingest-spool\n";
    assert!(!agent_command_unit_uses_stale_grammar(unit_text));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib setup::doctor_tests::stale_agent_command_grammar -- --nocapture`
Expected: FAIL to compile — `agent_command_unit_uses_stale_grammar` doesn't exist yet.

- [ ] **Step 4: Add the ExecStart-anchored detection function and the doctor phase to `src/setup/doctor.rs`**

Add near the other phase-building functions in this file (match the existing `use` imports already present):

```rust
/// Returns `true` when `unit_text` (the output of `systemctl --user cat
/// <unit>`) has an `ExecStart=` line invoking cortex's agent-command
/// spool-drain path using grammar older than the current canonical `ingest
/// shell agent index`. Anchored to `ExecStart=` specifically — using the
/// same basename+argv-shape approach as `is_agent_command_ingest_spool_invocation`
/// (see `src/command_log.rs`) — rather than a raw substring search over the
/// whole unit file, so a `Description=`/comment that merely *mentions* the
/// old grammar can never cause a false positive. Doesn't attempt to judge
/// whether the unit's target host is "correct" (that's an operator
/// decision); it only flags a stale, mechanically-detectable fact.
pub(crate) fn agent_command_unit_uses_stale_grammar(unit_text: &str) -> bool {
    unit_text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ExecStart="))
        .any(exec_start_uses_stale_grammar)
}

fn exec_start_uses_stale_grammar(exec_start: &str) -> bool {
    let tokens: Vec<&str> = exec_start.split_whitespace().collect();
    let Some(program) = tokens.first() else {
        return false;
    };
    let program_name = std::path::Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program);
    if program_name != "cortex" {
        return false;
    }
    let rest = &tokens[1..];
    let uses_current_grammar = matches!(rest, ["ingest", "shell", "agent", "index", ..]);
    let uses_stale_grammar = matches!(rest, ["ingest", "agent-command", "ingest-spool", ..])
        || matches!(rest, ["agent-command", "ingest-spool", ..]);
    uses_stale_grammar && !uses_current_grammar
}

/// Cheap pre-filter applied before `cat`-ing every discovered unit: a unit
/// whose *name* doesn't even plausibly relate to cortex/agent-command is
/// skipped without a `systemctl cat` call at all. On a host with hundreds of
/// unrelated systemd --user units (confirmed: 450 on the reference host this
/// task was reviewed against), catting every single one is wasteful even
/// once the scan is off the async runtime (see Step 5) — this bounds the
/// fan-out to plausible candidates without guessing at "the right host".
fn unit_name_plausibly_agent_command_related(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cortex") || lower.contains("agent") || lower.contains("command")
}

/// Scans `systemctl --user` service/timer units for ones whose `ExecStart=`
/// still invokes the pre-rename `agent-command ingest-spool` grammar, and
/// reports them so an operator can rerun `cortex setup shell agent install`
/// (which regenerates the wrapper) or manually fix the unit. Requires both
/// `fix: true` AND `yes: true` before disabling anything — matching this
/// repo's own `cortex compose down` precedent of refusing destructive action
/// without an explicit `--yes` — since a false positive here would silently
/// kill an unrelated running service. This is synchronous, blocking code:
/// callers MUST run it via `tokio::task::spawn_blocking` (see the async
/// wrapper below) rather than calling it directly from an async context.
fn stale_agent_command_units_scan(fix: bool, yes: bool) -> SetupPhase {
    let timer = PhaseTimer::start("stale-agent-command-units");
    let Some(unit_list) = super::systemd::systemctl_user_state("list-units", "--all") else {
        return timer.finish(SetupStatus::Ok, "systemctl --user unavailable; skipped");
    };
    let unit_names: Vec<&str> = unit_list
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.ends_with(".service") || name.ends_with(".timer"))
        .filter(|name| unit_name_plausibly_agent_command_related(name))
        .collect();

    let mut stale = Vec::new();
    for unit in unit_names {
        let Some(unit_text) = super::systemd::systemctl_user_state("cat", unit) else {
            tracing::debug!(
                unit,
                "systemctl --user cat failed; skipping stale-grammar check for this unit \
                 (result is inconclusive, not a confirmed clean bill of health)"
            );
            continue;
        };
        if agent_command_unit_uses_stale_grammar(&unit_text) {
            stale.push(unit.to_string());
        }
    }

    if stale.is_empty() {
        return timer.finish(
            SetupStatus::Ok,
            "no stale agent-command grammar found in systemd --user units",
        );
    }

    if fix && !yes {
        return timer.finish(
            SetupStatus::Warn,
            format!(
                "stale agent-command grammar in: {} — rerun with `cortex doctor --fix --yes` \
                 to disable, or fix/regenerate manually with `cortex setup shell agent install`",
                stale.join(", ")
            ),
        );
    }

    if fix {
        let mut disabled = Vec::new();
        let mut failed = Vec::new();
        for unit in &stale {
            match super::systemd::run_systemctl_user(&["disable", "--now", unit]) {
                Ok(output) if output.status.success() => disabled.push(unit.clone()),
                Ok(output) => failed.push(format!("{unit} (systemctl exited {})", output.status)),
                Err(error) => failed.push(format!("{unit} ({error})")),
            }
        }
        if failed.is_empty() {
            return timer.finish(
                SetupStatus::Warn,
                format!("disabled stale agent-command units: {}", disabled.join(", ")),
            );
        }
        return timer.finish(
            SetupStatus::Error,
            format!(
                "disabled {} unit(s) [{}]; FAILED to disable: {}",
                disabled.len(),
                disabled.join(", "),
                failed.join("; ")
            ),
        );
    }

    timer.finish(
        SetupStatus::Warn,
        format!(
            "stale agent-command grammar in: {} — run `cortex setup shell agent install` then \
             `cortex doctor --fix --yes`, or fix/disable manually",
            stale.join(", ")
        ),
    )
}

/// Async entry point: offloads the blocking systemd scan (subprocess spawns
/// via `Command::output()`, no timeout) onto the blocking thread pool so it
/// can never stall a Tokio worker thread, regardless of how many systemd
/// --user units the host has.
pub(crate) async fn stale_agent_command_units_phase(fix: bool, yes: bool) -> SetupPhase {
    let timer = PhaseTimer::start("stale-agent-command-units");
    match tokio::task::spawn_blocking(move || stale_agent_command_units_scan(fix, yes)).await {
        Ok(phase) => phase,
        Err(error) => timer.finish(
            SetupStatus::Error,
            format!("stale agent-command unit scan task panicked: {error}"),
        ),
    }
}
```

(The `run_systemctl_user` helper in `src/setup/systemd.rs` is currently `pub(super)`, not `pub(crate)` — check with `grep -n "fn run_systemctl_user" src/setup/systemd.rs`. If it's `pub(super)`, widen it to `pub(crate)` so `doctor.rs` — a sibling module under `setup`, not `systemd`'s direct parent — can call it directly; `pub(super)` only grants access to `setup.rs` itself, not to `setup::doctor`. Make this one-line visibility change as part of this step.)

- [ ] **Step 5: Wire the phase into `run_setup_doctor` and thread `fix`/`yes` parameters**

Find wherever `run_setup_doctor()` assembles its `phases` vector (from Step 1's read) and add:

```rust
phases.push(stale_agent_command_units_phase(fix, yes).await);
```

(this matches the existing pattern in this file of `.await`-ing other async phase-builders, e.g. `run_sessions_watch_service_setup(...).await?` — confirm with the Step 1 read.)

If `run_setup_doctor()` doesn't currently take `fix`/`yes` parameters, add them, and update its single call site in `src/main.rs` (`SetupCommandKind::Doctor => cortex::setup::run_setup_doctor().await?` becomes `cortex::setup::run_setup_doctor(doctor_fix, doctor_yes).await?`, where `doctor_fix`/`doctor_yes` come from new `--fix`/`--yes` flags parsed alongside the existing `--json` flag for the `doctor` command — check `src/main.rs`'s doctor-argument parsing block, likely near `DoctorBinaryCommand`, for the exact spot to add this parsing, following the same `while`-loop-over-args idiom already used elsewhere in this file for other commands).

- [ ] **Step 6: Update `help.rs`'s doctor usage line**

Find:

```rust
        usage: &["cortex doctor [--json]", "cortex doctor binary [--json]"],
```

Replace with:

```rust
        usage: &[
            "cortex doctor [--json] [--fix] [--yes]",
            "cortex doctor binary [--json]",
        ],
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --lib setup::doctor_tests -- --nocapture`
Expected: PASS — all five tests, including the new `stale_agent_command_grammar_ignores_mentions_outside_exec_start` and `stale_agent_command_grammar_ignores_non_cortex_binary` cases that the old raw-substring approach would have failed.

- [ ] **Step 8: Add a test proving `--fix` without `--yes` never disables anything**

Add to `src/setup/doctor_tests.rs` (this requires a way to inject a fake `stale` result without shelling out to real `systemctl` — check whether `stale_agent_command_units_scan`'s dependency on `super::systemd::systemctl_user_state` can be exercised in a test environment where `systemctl --user` is unavailable; if so, the function should return `SetupStatus::Ok` with "systemctl --user unavailable; skipped" per its own early-return, which doesn't exercise the fix/yes gating at all. In that case, test the gating logic directly by extracting the fix/yes decision into a small pure function, e.g. `fn should_disable(fix: bool, yes: bool, stale: &[String]) -> bool { fix && yes && !stale.is_empty() }`, unit-testable without any `systemctl` dependency):

```rust
#[test]
fn stale_agent_command_fix_requires_yes() {
    let stale = vec!["cortex-agent-command-ingest.timer".to_string()];
    assert!(!should_disable(true, false, &stale), "fix without yes must not disable");
    assert!(should_disable(true, true, &stale), "fix with yes should disable");
    assert!(!should_disable(false, true, &stale), "yes alone without fix must not disable");
}
```

Extract this `should_disable` helper in `doctor.rs` and use it in place of the inline `if fix && !yes` / `if fix` checks in `stale_agent_command_units_scan` so the gating logic itself has a test independent of `systemctl` availability.

- [ ] **Step 9: Run the full suite, clippy, and fmt one final time**

Run: `cargo build --lib --bin cortex 2>&1 | tail -60 && cargo test --lib 2>&1 | tail -100 && cargo fmt --check && RUSTC_WRAPPER='' cargo clippy --config 'build.rustc-wrapper=""' --all-targets -- -D warnings 2>&1 | tail -60`
Expected: all PASS. This is the plan's final checkpoint.

- [ ] **Step 10: Commit**

```bash
git add src/setup/doctor.rs src/setup/systemd.rs src/setup/doctor_tests.rs src/main.rs src/cli/help.rs
git commit -m "feat(doctor): detect and optionally disable stale agent-command systemd units"
```

---

## Post-implementation checklist (not a task — do this before opening the PR)

- [ ] `cargo xtask bump-version <patch|minor|major>` — this plan adds a new CLI grammar with a back-compat alias plus a new HTTP endpoint, which is additive (`feat`), so `minor` is the expected bump unless the actual commit history ends up dominated by `fix:` commits.
- [ ] Add a `CHANGELOG.md` entry under the bumped version summarizing: CLI rename (`ingest shell user|agent`, `setup shell agent`), new `setup shell completions`, agent-command forwarding (`--server`/`--token` on `ingest shell agent index`, new `/v1/agent-commands` endpoint), and `cortex doctor --fix` stale-unit detection.
- [ ] `cargo xtask check-version-sync` and `cargo xtask check-release-versions` both pass.
- [ ] Update `README.md`'s "Syslog Forwarder Setup"/CLI reference sections (search `grep -n "agent-command\|ingest-spool" README.md docs/*.md openwiki/*.md`) for any remaining stale grammar mentions this plan's `grep` sweeps didn't catch — the codebase-wide sweep in Task 8, Step 6 only covers `src/`, not `README.md`/`docs/`/`openwiki/`.
- [ ] On any host with an already-installed wrapper or manually-created timer (confirmed: `devhost` has one), after deploying this change run `cortex setup shell agent install` to regenerate the wrapper, and `cortex doctor --fix` (or manual `systemctl --user cat <unit>` review) to catch and retire the stale timer this whole plan exists to fix in the first place.
- [ ] Close beads `syslog-mcp-4n4a6`'s remaining follow-up decision (or open two fresh beads if you'd rather track "forwarding" and "stale-timer detection" as separate trackable units retroactively) once this lands and is verified on `devhost`.
