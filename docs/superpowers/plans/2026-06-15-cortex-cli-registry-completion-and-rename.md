# Cortex CLI: Registry Metadata + Dynamic Completion + Canonical Rename — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the cortex CLI discoverable and forgiving: tab-completion that knows every action, its flags, and *live values* (hostnames, apps, source IPs); a single canonical flag vocabulary across CLI and MCP; and "no-help help" generated from one registry.

**Architecture:** Extend the existing `ACTION_SPECS` registry (`src/mcp/actions.rs`) with per-action flag and example metadata, so the CLI parser, completion engine, help overview, and MCP schema all derive from one source. Add a hidden `cortex __complete` command that emits completion candidates (static from the registry, dynamic from the DB with a short cache + hard timeout) and a `cortex completions zsh` generator. Perform a clean rename of flag/param names to the canonical vocabulary across the CLI parse layer and the MCP tool schema. This is Plan 2 of 3; it depends on Plan 1's time parser (`src/cli/timearg.rs`).

**Tech Stack:** Rust (edition 2024), the existing `FlagCursor` parse helpers, `schemas::tool_definitions()` (MCP JSON schema), `help.rs` section map, existing DB list queries (`list_hosts`/`list_apps`/`list_source_ips`), zsh completion.

**Spec:** `docs/superpowers/specs/2026-06-15-cortex-cli-ergonomics-design.md` (Components 1, 2, 5, 7).

---

## Naming reality (read before starting)

- CLI command names are hyphenated (`source-ips`, `host-state`); MCP action names are
  underscored (`source_ips`, `host_state`). `ACTION_SPECS[].name` holds the MCP form.
  The completion + help work keys off the **CLI** command list in
  `src/cli/parse.rs::TOP_LEVEL_COMMANDS`; the canonical-flag metadata is shared.
- The clean rename changes **flag/param names only**, not action/command names.

### Canonical flag vocabulary (the rename target)

| Concept | Canonical | Old CLI flag | Old MCP property |
|---|---|---|---|
| host | `--host` | `--host` | `hostname` |
| literal text | `--grep` | (Plan 1) | (n/a) |
| limit | `-n`, `--limit` | `--limit` | `limit` |
| min severity | `-s`, `--severity` | `--severity` | `severity` |
| app | `--app` | `--app` | `app_name` |
| source id | `--source` | `--source` | `source_ip` |
| event-time start | `--since` | `--from` | `from` |
| event-time end | `--until` | `--to` | `to` |
| received start | `--received-since` | `--received-since` | `received_from` |
| received end | `--received-until` | `--received-until` | `received_to` |

`--container`, `--stream`, `--source-kind`, `--json`, `--facility`,
`--exclude-facility` keep their names.

---

## File Structure

- **Modify** `src/mcp/actions.rs` — extend `ActionSpec` with `flags: &'static [FlagSpec]`
  and `examples: &'static [&'static str]`; add a `FlagSpec` type; extend the
  `action_spec!` macro. New accessors `flags_for(action)` and `examples_for(action)`.
- **Create** `src/mcp/action_flags.rs` — the `FlagSpec` type + the canonical flag
  constant groups (a `COMMON_LOG_FLAGS` slice reused by search/filter/tail/etc.) so
  flag metadata is DRY.
- **Create** `src/cli/complete.rs` + `src/cli/complete_tests.rs` — the `__complete`
  candidate engine (static + dynamic + cache).
- **Create** `src/cli/completions/zsh.rs` (or a `completions.rs` with an embedded
  script) — the `cortex completions zsh` generator.
- **Modify** `src/cli/parse.rs` — register `__complete` and `completions` commands.
- **Modify** the per-action `parse_*` fns (`src/cli/parse_logs.rs`, `parse_ai*.rs`,
  `parse_admin.rs`, `commands/*.rs`) — rename flags to canonical names.
- **Modify** `src/mcp/schemas.rs` + `src/mcp/tools.rs` — rename MCP properties and the
  arg extraction to canonical names.
- **Modify** `src/cli/help.rs` — generate the no-args overview action list and
  missing-arg usage from `ACTION_SPECS` descriptions + examples.
- **Modify** the 9 plugin skills, `CLAUDE.md`, `docs/mcp/*.md`, `README`,
  `scripts/smoke-test.sh`, `config/mcporter.json` — canonical names.

---

## Task 1: Extend `ActionSpec` with flag + example metadata

**Files:**
- Create: `src/mcp/action_flags.rs`
- Modify: `src/mcp/actions.rs`
- Test: `src/mcp/actions_tests.rs` (create if absent; wire `#[cfg(test)] #[path] mod tests;` in `actions.rs`)

- [ ] **Step 1: Define `FlagSpec` and canonical flag groups**

Create `src/mcp/action_flags.rs`:

```rust
//! Canonical CLI flag metadata, shared by the parser, completion, and help.

/// One CLI flag for an action. `value_kind` drives dynamic completion.
#[derive(Debug, Clone, Copy)]
pub(super) struct FlagSpec {
    /// Canonical long flag, including leading dashes, e.g. "--host".
    pub flag: &'static str,
    /// Optional short alias, e.g. "-n". Empty string = none.
    pub short: &'static str,
    /// One-line help.
    pub help: &'static str,
    /// Completion source for the flag's value.
    pub value_kind: ValueKind,
}

/// What completes after a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ValueKind {
    /// No value (boolean flag).
    None,
    /// Free text (no candidates).
    Text,
    /// Live hostnames from the DB.
    Host,
    /// Live app names from the DB.
    App,
    /// Live source identifiers from the DB.
    Source,
    /// Fixed enum candidates.
    Enum(&'static [&'static str]),
    /// A time value (offers relative hints).
    Time,
}

pub(super) const SEVERITIES: &[&str] =
    &["emerg", "alert", "crit", "err", "warning", "notice", "info", "debug"];

/// Flags shared by the log-query actions (search/filter/tail/errors/...).
pub(super) const COMMON_LOG_FLAGS: &[FlagSpec] = &[
    FlagSpec { flag: "--host", short: "", help: "Filter by hostname", value_kind: ValueKind::Host },
    FlagSpec { flag: "--app", short: "", help: "Filter by app/program name", value_kind: ValueKind::App },
    FlagSpec { flag: "--source", short: "", help: "Filter by source id (IP:port or docker://...)", value_kind: ValueKind::Source },
    FlagSpec { flag: "--severity", short: "-s", help: "Minimum severity", value_kind: ValueKind::Enum(SEVERITIES) },
    FlagSpec { flag: "--since", short: "", help: "Start of window (1h, 2d, yesterday, RFC3339)", value_kind: ValueKind::Time },
    FlagSpec { flag: "--until", short: "", help: "End of window", value_kind: ValueKind::Time },
    FlagSpec { flag: "--limit", short: "-n", help: "Max results", value_kind: ValueKind::Text },
    FlagSpec { flag: "--json", short: "", help: "JSON output", value_kind: ValueKind::None },
];
```

- [ ] **Step 2: Write the failing test**

Create `src/mcp/actions_tests.rs`:

```rust
use super::*;

#[test]
fn every_action_has_nonempty_description() {
    for spec in ACTION_SPECS {
        assert!(!spec.description.is_empty(), "{} missing description", spec.name);
    }
}

#[test]
fn search_action_exposes_common_flags_and_examples() {
    let flags = flags_for("search").expect("search has flags");
    assert!(flags.iter().any(|f| f.flag == "--host"));
    assert!(flags.iter().any(|f| f.flag == "--since"));
    let ex = examples_for("search").expect("search has examples");
    assert!(!ex.is_empty(), "search should ship at least one example");
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p cortex --lib actions::tests`
Expected: FAIL — `flags_for`/`examples_for` undefined; `ActionSpec` has no `flags`/`examples`.

- [ ] **Step 4: Extend `ActionSpec`, the macro, and add accessors**

In `src/mcp/actions.rs`:
- Add `mod action_flags;` and `use action_flags::{FlagSpec, COMMON_LOG_FLAGS};` (plus `ValueKind` where needed).
- Add two fields to `struct ActionSpec`:

```rust
    /// CLI flags for this action (canonical names).
    pub flags: &'static [FlagSpec],
    /// Copy-paste example invocations.
    pub examples: &'static [&'static str],
```

- Update the `action_spec!` macro to accept the two extra arguments. Keep the
  existing 5-arg form working by adding a second macro arm with defaults:

```rust
macro_rules! action_spec {
    // Full form.
    ($name:literal, $scope:ident, $description:literal, $cost:ident, $handler:ident,
     flags: $flags:expr, examples: $examples:expr) => {
        ActionSpec {
            name: $name, scope: Scope::$scope, description: $description,
            cost: Cost::$cost, handler: ActionHandler::$handler,
            flags: $flags, examples: $examples,
        }
    };
    // Short form: no flags/examples yet.
    ($name:literal, $scope:ident, $description:literal, $cost:ident, $handler:ident) => {
        action_spec!($name, $scope, $description, $cost, $handler, flags: &[], examples: &[])
    };
}
```

- Populate `search` (and `filter`, `tail`, `errors`) using the full form:

```rust
    action_spec!(
        "search", Read, "Full-text search over syslog messages", Cheap, SearchLogs,
        flags: COMMON_LOG_FLAGS,
        examples: &[
            "cortex search \"oom killer\" --host devhost --since 1h",
            "cortex search --grep \"smoke-test\" --limit 20",
        ]
    ),
```

- Add accessors near `handler_for`:

```rust
pub(super) fn flags_for(action: &str) -> Option<&'static [FlagSpec]> {
    ACTION_SPECS.iter().find(|s| s.name == action).map(|s| s.flags)
}
pub(super) fn examples_for(action: &str) -> Option<&'static [&'static str]> {
    ACTION_SPECS.iter().find(|s| s.name == action).map(|s| s.examples)
}
```

- Wire the sidecar test at the end of `actions.rs`:

```rust
#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
```

> Visibility note: `flags_for`/`examples_for` are `pub(super)` (mcp module). The CLI
> lives in a different module, so also add thin re-exports the CLI can call — expose
> `pub fn cli_flags_for(action: &str) -> &'static [FlagSpec]` and
> `pub fn cli_examples_for(...)` from a small `pub` facade (e.g. `src/mcp/registry.rs`
> or make these `pub` in `actions` and re-export at `src/mcp.rs`). Pick the path that
> matches how `action_names()` is already exposed (`rg -n "action_names|pub fn" src/mcp.rs`).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p cortex --lib actions::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/action_flags.rs src/mcp/actions.rs src/mcp/actions_tests.rs
git commit -m "feat(mcp): add canonical flag + example metadata to ACTION_SPECS"
```

- [ ] **Step 7: Backfill metadata for the remaining log-query actions**

Repeat Step 4's full-form population for the remaining actions that take the common
log flags (`filter`, `tail`, `errors`, `timeline`, `patterns`, `context`,
`correlate`, `source_ips`, `apps`, `hosts`, `silent_hosts`, `clock_skew`,
`anomalies`, `compare`). For actions with extra flags, append action-specific
`FlagSpec`s to a per-action slice rather than `COMMON_LOG_FLAGS`. Add at least one
`example` per action. Add a test asserting **every** action that the CLI exposes has
≥1 example:

```rust
#[test]
fn all_cli_query_actions_have_examples() {
    for name in ["search","filter","tail","errors","hosts","apps","timeline",
                 "patterns","correlate","source_ips","stats","status"] {
        assert!(examples_for(name).map(|e| !e.is_empty()).unwrap_or(false),
            "{name} needs an example");
    }
}
```

Run: `cargo test -p cortex --lib actions::tests` → PASS. Commit
`feat(mcp): backfill flag/example metadata for log-query actions`.

---

## Task 2: Canonical flag rename — CLI parse layer (mechanical sweep)

This is a **mechanical rename**, not 46 distinct designs. Apply the vocabulary table
to every `parse_*` fn. One worked example + a completeness gate keeps it honest.

**Files (the sweep set — from `parse.rs` dispatch):**
- `src/cli/parse_logs.rs`, `src/cli/parse_ai.rs`, `src/cli/parse_ai_more.rs`,
  `src/cli/parse_admin.rs`, `src/cli/commands/*.rs`
- Their sidecar `*_tests.rs`

- [ ] **Step 1: Worked example (search)**

In `src/cli/parse_logs.rs::parse_search`, rename per the table. Before:

```rust
"--host" => parsed.hostname = Some(flags.value("--host")?),
"--source" => parsed.source_ip = Some(flags.value("--source")?),
"--app" => parsed.app_name = Some(flags.value("--app")?),
"--since" => parsed.from = Some(norm_time(flags.value("--since")?)?),
"--until" => parsed.to = Some(norm_time(flags.value("--until")?)?),
```

After (note `-s`/`-n` short forms and `--since/--until`):

```rust
"--host" => parsed.hostname = Some(flags.value("--host")?),
"--source" => parsed.source_ip = Some(flags.value("--source")?),
"--app" => parsed.app_name = Some(flags.value("--app")?),
"--since" => parsed.from = Some(norm_time(flags.value("--since")?)?),
"--until" => parsed.to = Some(norm_time(flags.value("--until")?)?),
"-s" | "--severity" => parsed.severity = Some(flags.value("--severity")?),
"-n" | "--limit" => parsed.limit = Some(parse_u32_flag("--limit", flags.value("--limit")?)?),
```

Rename the `--flag=value` arms and the `unknown_option` candidate lists to match.
(Struct field names like `parsed.hostname` stay — only the user-facing flag text changes.)

- [ ] **Step 2: Update that fn's tests, run, confirm green**

Update `parse_logs_tests.rs` cases that used old flag names to canonical names; add
one asserting the old name now errors:

```rust
#[test]
fn search_rejects_legacy_hostname_flag() {
    let err = parse_search(&["x".into(), "--host".into(), "devhost".into()])
        .unwrap_err().to_string();
    assert!(err.contains("--host"), "should suggest canonical flag: {err}");
}
```

Run: `cargo test -p cortex --lib parse_logs` → PASS.

- [ ] **Step 3: Sweep the remaining parse fns**

Apply the identical table transformation to every other `parse_*` fn that accepts the
renamed flags. Work file-by-file; after each file run its sidecar tests.

- [ ] **Step 4: Completeness gate (no old flag names remain)**

Run:

```bash
rg -n -- '--host|--source|--app|"--since"|"--until"|--received-since|--received-until' src/cli
```

Expected: **no matches in `parse_*`/command code** (matches only allowed in help text
that is itself being rewritten in Task 5, and in comments). Fix any stragglers.

- [ ] **Step 5: Run the full CLI test suite**

Run: `cargo test -p cortex --lib cli` → PASS.

- [ ] **Step 6: Commit**

```bash
git add src/cli
git commit -m "feat(cli): rename query flags to canonical vocabulary (clean break)"
```

---

## Task 3: Canonical rename — MCP schema + arg extraction

**Files:**
- Modify: `src/mcp/schemas.rs` (property names + descriptions)
- Modify: `src/mcp/tools.rs` (arg extraction reads the new property names)
- Modify: `docs/mcp/SCHEMA.md`, `docs/mcp/TOOLS.md` (regenerate/rewrite)
- Test: `src/mcp/schemas_tests.rs` or `tools_tests.rs`

- [ ] **Step 1: Write the failing test**

In the MCP schema test file:

```rust
#[test]
fn schema_uses_canonical_property_names() {
    let defs = tool_definitions();
    let props = &defs[0]["inputSchema"]["properties"]; // adjust path to match tool_definitions output
    assert!(props.get("host").is_some(), "canonical 'host' property present");
    assert!(props.get("hostname").is_none(), "legacy 'hostname' removed");
    assert!(props.get("since").is_some() && props.get("from").is_none());
}
```

(Adjust the JSON path to match what `tool_definitions()` returns — `rg -n "inputSchema|properties" src/mcp/schemas.rs`.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib schema_uses_canonical_property_names`
Expected: FAIL — `hostname`/`from` still present.

- [ ] **Step 3: Rename properties in `schemas.rs`**

Rename JSON property keys per the table: `hostname`→`host`, `source_ip`→`source`,
`app_name`→`app`, `from`→`since`, `to`→`until`, `received_from`→`received_since`,
`received_to`→`received_until`. Update the descriptions' inline references too.

- [ ] **Step 4: Update arg extraction in `tools.rs`**

`rg -n '"hostname"|"source_ip"|"app_name"|"from"|"to"|"received_from"|"received_to"' src/mcp/tools.rs`
and rename each `arguments.get("...")` key to the canonical name.

- [ ] **Step 5: Completeness gate + tests**

```bash
rg -n '"hostname"|"source_ip"|"app_name"|"received_from"|"received_to"' src/mcp/schemas.rs src/mcp/tools.rs
```
Expected: no matches. Run `cargo test -p cortex --lib mcp` → PASS.

- [ ] **Step 6: Regenerate the action table + schema docs**

Run the repo's schema/table generation if one exists (`rg -n "SCHEMA.md|TOOLS.md|generate" justfile scripts`); otherwise hand-update `docs/mcp/SCHEMA.md` and `docs/mcp/TOOLS.md` to canonical names.

- [ ] **Step 7: Commit**

```bash
git add src/mcp/schemas.rs src/mcp/tools.rs docs/mcp
git commit -m "feat(mcp): rename tool arguments to canonical vocabulary (clean break)"
```

---

## Task 4: `cortex __complete` engine — static candidates

**Files:**
- Create: `src/cli/complete.rs`, `src/cli/complete_tests.rs`
- Modify: `src/cli/parse.rs` (route `__complete`), `src/cli.rs` (register module),
  `src/cli/run.rs` (dispatch `__complete` to a printer)

- [ ] **Step 1: Write the failing test (static actions + flags)**

Create `src/cli/complete_tests.rs`:

```rust
use super::*;

#[test]
fn completes_action_names_with_descriptions() {
    let out = complete(&["actions".into()]).unwrap();
    assert!(out.iter().any(|line| line.starts_with("search\t")));
    assert!(out.iter().any(|line| line.starts_with("tail\t")));
}

#[test]
fn completes_flags_for_action() {
    let out = complete(&["flags".into(), "search".into()]).unwrap();
    assert!(out.iter().any(|l| l.starts_with("--host\t")));
    assert!(out.iter().any(|l| l.starts_with("--since\t")));
    assert!(out.iter().any(|l| l.starts_with("--grep\t")));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib complete::tests`
Expected: FAIL — `complete` undefined.

- [ ] **Step 3: Implement static completion**

Create `src/cli/complete.rs`:

```rust
//! Candidate generator for shell completion. `cortex __complete <ctx>` prints
//! one `value\tdescription` line per candidate.

use anyhow::{Result, bail};

/// Top-level completion entry. `args[0]` is the context kind.
pub(crate) fn complete(args: &[String]) -> Result<Vec<String>> {
    let (kind, rest) = args.split_first().map(|(k, r)| (k.as_str(), r))
        .ok_or_else(|| anyhow::anyhow!("completion context required"))?;
    match kind {
        "actions" => Ok(action_candidates()),
        "flags" => {
            let action = rest.first().map(|s| s.as_str()).unwrap_or("");
            Ok(flag_candidates(action))
        }
        "value" => complete_value(rest), // dynamic; Task 5
        other => bail!("unknown completion context '{other}'"),
    }
}

fn action_candidates() -> Vec<String> {
    // CLI command names with one-line descriptions from the registry.
    crate::cli::registry_actions()
        .iter()
        .map(|(name, desc)| format!("{name}\t{desc}"))
        .collect()
}

fn flag_candidates(action: &str) -> Vec<String> {
    let mut out = Vec::new();
    for f in crate::cli::registry_flags(action) {
        out.push(format!("{}\t{}", f.flag, f.help));
        if !f.short.is_empty() {
            out.push(format!("{}\t{}", f.short, f.help));
        }
    }
    out
}

fn complete_value(_rest: &[String]) -> Result<Vec<String>> {
    // Implemented in Task 5.
    Ok(Vec::new())
}
```

Add to `src/cli.rs`: `pub(crate) mod complete;` and the sidecar hook in `complete.rs`:

```rust
#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
```

Provide the `registry_actions()` / `registry_flags()` facade in `src/cli.rs` (or a
small `cli/registry.rs`) that maps CLI command names → MCP action names and returns
the metadata from Task 1. Use `parse::TOP_LEVEL_COMMANDS` for the CLI name list and a
hyphen↔underscore converter to look up the spec.

- [ ] **Step 4: Route the hidden command**

In `src/cli/parse.rs::parse_command`, add before the fallthrough:

```rust
"__complete" => Ok(CliCommand::Complete(rest.to_vec())),
"completions" => Ok(CliCommand::Completions(rest.to_vec())), // Task 6
```

Add the `CliCommand::Complete(Vec<String>)` and `Completions(Vec<String>)` variants
(wherever `CliCommand` is defined — `rg -n "enum CliCommand" src/cli`), and in
`src/cli/run.rs` dispatch `Complete` to print `complete(&args)?` lines to stdout
(no DB needed for static; dynamic is added in Task 5). Keep `__complete` out of
`TOP_LEVEL_COMMANDS` and out of help (hidden).

- [ ] **Step 5: Run tests**

Run: `cargo test -p cortex --lib complete::tests` → PASS.

- [ ] **Step 6: Commit**

```bash
git add src/cli/complete.rs src/cli/complete_tests.rs src/cli.rs src/cli/parse.rs src/cli/run.rs
git commit -m "feat(cli): cortex __complete engine (static action+flag candidates)"
```

---

## Task 5: `__complete` dynamic values (hosts/apps/sources) + cache + timeout

**Files:**
- Modify: `src/cli/complete.rs`, `src/cli/complete_tests.rs`
- Reuse: existing DB list queries (`rg -n "fn list_hosts|fn list_apps|fn list_source_ips" src/db`)

- [ ] **Step 1: Write the failing test (enum values are static + deterministic)**

```rust
#[test]
fn completes_static_enum_values() {
    // severity is a fixed enum; no DB needed.
    let out = complete(&["value".into(), "--severity".into()]).unwrap();
    assert!(out.iter().any(|l| l.starts_with("err")));
    assert!(out.iter().any(|l| l.starts_with("warning")));
}
```

- [ ] **Step 2: Run, confirm fail, implement enum + dynamic dispatch**

Run the test (FAIL). Implement `complete_value`:

```rust
use crate::mcp::action_flags::ValueKind;

fn complete_value(rest: &[String]) -> Result<Vec<String>> {
    let flag = rest.first().map(|s| s.as_str()).unwrap_or("");
    match value_kind_for_flag(flag) {
        ValueKind::Enum(items) => Ok(items.iter().map(|s| s.to_string()).collect()),
        ValueKind::Host => dynamic_cached("host", load_hosts),
        ValueKind::App => dynamic_cached("app", load_apps),
        ValueKind::Source => dynamic_cached("source", load_sources),
        ValueKind::Time => Ok(time_hints()),
        _ => Ok(Vec::new()),
    }
}

fn time_hints() -> Vec<String> {
    ["15m","30m","1h","6h","1d","2d","yesterday","today"]
        .iter().map(|s| s.to_string()).collect()
}
```

`value_kind_for_flag` scans all `FlagSpec`s for a matching flag (the flag→kind map is
unambiguous across the canonical vocabulary).

- [ ] **Step 3: Implement the cache + timeout wrapper**

```rust
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_TTL_SECS: u64 = 60;

/// Read a kind's candidates from a tmp cache; refresh via `load` on miss/expiry.
/// Any error (DB unreachable, timeout) yields an empty Vec so completion silently
/// degrades to static candidates.
fn dynamic_cached(kind: &str, load: fn() -> Result<Vec<String>>) -> Result<Vec<String>> {
    let path = cache_path(kind);
    if let Some(fresh) = read_fresh(&path, CACHE_TTL_SECS) {
        return Ok(fresh);
    }
    match load() {
        Ok(values) => {
            let _ = write_cache(&path, &values);
            Ok(values)
        }
        Err(_) => Ok(read_any(&path).unwrap_or_default()), // stale-but-usable, else empty
    }
}
```

`cache_path` lives under `$XDG_RUNTIME_DIR/cortex-complete/` (fallback `std::env::temp_dir()`).
`load_hosts/apps/sources` call the existing list queries through a **short-lived,
read-only** DB handle with a hard query timeout (e.g. `rusqlite` `busy_timeout(150)`
and a `LIMIT` on candidates); on any failure they return `Err`, which the wrapper
swallows. Mark these as bounded: cap at e.g. 500 candidates, most-recent-first.

> **No silent over-cap:** when candidates are capped, that's acceptable for completion
> (you only see the top matches); document the cap in the function doc-comment.

- [ ] **Step 4: Test the degrade-to-empty path**

```rust
#[test]
fn dynamic_value_degrades_to_empty_without_db() {
    // With CORTEX_DB_PATH pointed at a nonexistent file, host completion must not error.
    let _g = EnvGuard::set("CORTEX_DB_PATH", "/nonexistent/cortex.db");
    let out = complete(&["value".into(), "--host".into()]).unwrap();
    assert!(out.is_empty() || out.iter().all(|l| !l.is_empty()));
}
```

(Use the repo's `EnvGuard` test helper — `rg -n "struct EnvGuard|fn set" src/cli`.)

- [ ] **Step 5: Run tests + commit**

Run: `cargo test -p cortex --lib complete` → PASS.

```bash
git add src/cli/complete.rs src/cli/complete_tests.rs
git commit -m "feat(cli): dynamic completion values (hosts/apps/sources) with cache + degrade"
```

---

## Task 6: `cortex completions zsh` generator + zsh function

**Files:**
- Create: `src/cli/completions.rs` (embeds the zsh script via `include_str!`)
- Create: `src/cli/completions/_cortex.zsh` (the script)
- Modify: `src/cli/run.rs` (dispatch `Completions`)
- Test: `src/cli/completions_tests.rs` (assert the script references `__complete`)

- [ ] **Step 1: Write the zsh completion function**

Create `src/cli/completions/_cortex.zsh`:

```zsh
#compdef cortex
# cortex zsh completion — delegates to `cortex __complete`.
_cortex() {
  local -a candidates
  local context state line
  local cur=${words[CURRENT]}
  local prev=${words[CURRENT-1]}

  # First word: complete action names.
  if (( CURRENT == 2 )); then
    candidates=("${(@f)$(cortex __complete actions 2>/dev/null)}")
    _describe -t actions 'cortex action' candidates
    return
  fi

  local action=${words[2]}

  # If the previous word is a flag that takes a value, complete the value.
  if [[ $prev == --* || $prev == -[a-z] ]]; then
    local vals
    vals=("${(@f)$(cortex __complete value $prev 2>/dev/null)}")
    if (( ${#vals} )); then
      compadd -- ${vals%%$'\t'*}
      return
    fi
  fi

  # Otherwise complete flags for the action.
  candidates=("${(@f)$(cortex __complete flags $action 2>/dev/null)}")
  _describe -t flags 'flag' candidates
}
_cortex "$@"
```

- [ ] **Step 2: Write the failing test**

Create `src/cli/completions_tests.rs`:

```rust
use super::*;

#[test]
fn zsh_script_is_emitted_and_calls_complete() {
    let script = zsh_completion_script();
    assert!(script.contains("#compdef cortex"));
    assert!(script.contains("cortex __complete actions"));
}
```

- [ ] **Step 3: Implement the generator**

Create `src/cli/completions.rs`:

```rust
//! Shell completion script generation.

pub(crate) fn zsh_completion_script() -> &'static str {
    include_str!("completions/_cortex.zsh")
}

/// Print the script for `shell`, or an error for unsupported shells.
pub(crate) fn print_completions(shell: &str) -> anyhow::Result<()> {
    match shell {
        "zsh" => {
            println!("{}", zsh_completion_script());
            Ok(())
        }
        other => anyhow::bail!("unsupported shell '{other}'; supported: zsh"),
    }
}

#[cfg(test)]
#[path = "completions_tests.rs"]
mod tests;
```

Register `pub(crate) mod completions;` in `src/cli.rs`, and in `src/cli/run.rs`
dispatch `CliCommand::Completions(args)` → `completions::print_completions(args.first()...)`.

- [ ] **Step 4: Run tests + manual check**

Run: `cargo test -p cortex --lib completions` → PASS.
Manual: `cortex completions zsh | head -1` prints `#compdef cortex`.

- [ ] **Step 5: Commit**

```bash
git add src/cli/completions.rs src/cli/completions/_cortex.zsh src/cli/completions_tests.rs src/cli.rs src/cli/run.rs
git commit -m "feat(cli): cortex completions zsh — delegating completion function"
```

---

## Task 7: Discoverability surface (registry-driven help)

**Files:**
- Modify: `src/cli/help.rs` (overview action list + per-action examples from registry)
- Modify: per-action `parse_*` fns — on missing required arg, print short usage +
  examples instead of a terse error
- Test: `src/cli/help_tests.rs`

- [ ] **Step 1: Write the failing test (overview pulls descriptions from registry)**

In `src/cli/help_tests.rs`:

```rust
#[test]
fn overview_lists_actions_with_registry_descriptions() {
    let body = render_overview(false); // no color
    assert!(body.contains("search"));
    assert!(body.contains("Full-text search over syslog messages"));
    assert!(body.contains("cortex search")); // an example line
}
```

- [ ] **Step 2: Run to confirm failure, then implement**

Generate the grouped overview from the existing section map but pull each command's
one-line description from `examples_for`/the registry description rather than the
hand-written strings; append a short "Examples" block built from `examples_for` for a
few headline actions. Keep Aurora color tokens + `NO_COLOR` handling already in
`help.rs`.

- [ ] **Step 3: Missing-arg usage**

For actions with a required positional/flag (e.g. `search` needs a query or `--grep`),
when it's absent, print: one usage line + the action's `examples_for` entries, then
exit non-zero. Add a test:

```rust
#[test]
fn search_without_query_shows_examples() {
    let err = super::super::parse_logs::parse_search(&[]).unwrap_err().to_string();
    assert!(err.contains("cortex search"), "should show an example: {err}");
}
```

- [ ] **Step 4: Run help tests (update snapshots if present)**

Run: `cargo test -p cortex --lib help` → PASS (update snapshot files if the suite uses them).

- [ ] **Step 5: Commit**

```bash
git add src/cli/help.rs src/cli/help_tests.rs src/cli/parse_logs.rs
git commit -m "feat(cli): registry-driven overview + missing-arg examples"
```

---

## Task 8: Skills / docs / smoke migration to canonical names

**Files:**
- Modify: `plugins/cortex/skills/**/SKILL.md` (and the `cortex-*` skills) example invocations
- Modify: `CLAUDE.md`, `README*`, `docs/mcp/*.md`, `config/mcporter.json`,
  `scripts/smoke-test.sh`

- [ ] **Step 1: Find every legacy name reference**

```bash
rg -n -- '--host|--source|--app|action=.*hostname=|"hostname"|"source_ip"|"app_name"| from=| to=' \
  plugins CLAUDE.md README* docs scripts config
```

- [ ] **Step 2: Rewrite to canonical names**

Update each hit: CLI examples → `--host/--app/--source/--since/--until`; MCP-arg
examples → `host/app/source/since/until`. Keep semantics identical.

- [ ] **Step 3: Update the action/flag table in `CLAUDE.md`**

Regenerate the MCP action table and add the canonical-flag table reference.

- [ ] **Step 4: Run the smoke test against a live server**

Run: `bash scripts/smoke-test.sh`
Expected: PASS (all actions exercised with canonical flags).

- [ ] **Step 5: Commit**

```bash
git add plugins CLAUDE.md README* docs scripts config
git commit -m "docs: migrate skills/docs/smoke to canonical cortex flag vocabulary"
```

---

## Task 9: Final verification

- [ ] **Step 1: Gates**

Run: `cargo test -p cortex` → PASS.
Run: `cargo clippy --all-targets -- -D warnings` → clean.
Run: `cargo fmt --check` → clean.

- [ ] **Step 2: Install + drive completion manually**

```bash
cortex completions zsh > "${fpath[1]}/_cortex"   # or source into a test fpath
# new shell:
cortex <TAB>            # lists actions with descriptions
cortex search --<TAB>   # lists --host --since --grep ...
cortex search --host <TAB>   # lists live hostnames (devhost, nashost, ...)
```

- [ ] **Step 3: Commit any cleanup**

```bash
git add -A && git commit -m "chore(cli): clippy/fmt cleanup for completion + rename"
```

---

## Self-Review

**Spec coverage:** Component 1 (dynamic completion) → Tasks 4–6. Component 2 (canonical
rename) → Tasks 2–3 (+ metadata in Task 1). Component 5 (discoverability) → Task 7.
Component 7 (migration) → Task 8. Smart defaults/positionals (Component 4) → Plan 3.

**Placeholder scan:** Novel code (FlagSpec, complete engine, cache, zsh script,
generator) is complete. Tasks 2/3/8 are deliberately specified as *mechanical sweeps*
with a worked example + a `rg` completeness gate rather than N near-identical blocks —
the correct granularity for a rename across 46 actions; the gate proves completion.
Several steps end with a `rg -n` lookup because the exact line/struct location
(`enum CliCommand`, `EnvGuard`, list-query fns, `inputSchema` JSON path) wasn't read
during planning; each names the symbol to find and the change to make.

**Type consistency:** `FlagSpec{flag,short,help,value_kind}`, `ValueKind`,
`flags_for`/`examples_for`, `complete(&[String]) -> Result<Vec<String>>`,
`zsh_completion_script() -> &'static str`, `CliCommand::Complete/Completions` — used
consistently across tasks.

**Risks:** (1) the `pub(super)` visibility of registry accessors needs a small public
facade for the CLI module — flagged in Task 1 Step 4. (2) MCP arg rename breaks
external callers — acceptable (pre-prod), gated by Task 8 migrating the in-repo skills.
(3) dynamic completion latency — bounded by the 150 ms query timeout + 60 s cache +
500-candidate cap.
