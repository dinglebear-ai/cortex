# Cortex CLI: Smart Defaults + Positionals — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the common case flagless — `cortex tail devhost`, `cortex search "oom"`, `cortex host-state devhost` — and give each action sensible zero-flag defaults so you rarely need to type a flag at all.

**Architecture:** Add two pieces of metadata to the `ACTION_SPECS` registry: a `positional` mapping (which canonical flag a bare argument binds to) and a `defaults` block (limit / time-window applied when the user omits them). The parse layer reads this metadata via small shared helpers, so behavior is declarative, not per-command special-casing. This is Plan 3 of 3; it depends on Plan 2 (registry flag metadata) and Plan 1 (time parser).

**Tech Stack:** Rust (edition 2024), the `ActionSpec`/`FlagSpec` metadata from Plan 2, the `FlagCursor` parse helpers, `parse_time_arg` from Plan 1.

**Spec:** `docs/superpowers/specs/2026-06-15-cortex-cli-ergonomics-design.md` (Component 4).

---

## File Structure

- **Modify** `src/mcp/actions.rs` / `src/mcp/action_flags.rs` — add `positional` and
  `defaults` to the registry; add accessors `positional_for` / `defaults_for`.
- **Modify** the per-action `parse_*` fns (`src/cli/parse_logs.rs`, `commands/*.rs`) —
  bind a leftover positional via a shared helper; apply defaults after parsing.
- **Create** `src/cli/argdefaults.rs` + `src/cli/argdefaults_tests.rs` — the shared
  positional-binding + default-application helpers.

---

## Task 1: Registry metadata for positionals + defaults

**Files:**
- Modify: `src/mcp/action_flags.rs` (add `Defaults` type), `src/mcp/actions.rs`
- Test: `src/mcp/actions_tests.rs`

- [ ] **Step 1: Add the metadata types**

In `src/mcp/action_flags.rs`:

```rust
/// Zero-flag defaults applied when the user omits them.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Defaults {
    /// Default `--limit` when unset (None = leave to server default).
    pub limit: Option<u32>,
    /// Default `--since` window when unset, e.g. "1h" (None = unbounded).
    pub since: Option<&'static str>,
}
```

- [ ] **Step 2: Write the failing test**

In `src/mcp/actions_tests.rs`:

```rust
#[test]
fn tail_binds_positional_to_host_and_defaults_limit() {
    assert_eq!(positional_for("tail"), Some("--host"));
    assert_eq!(defaults_for("tail").limit, Some(50));
}

#[test]
fn search_positional_binds_to_query() {
    assert_eq!(positional_for("search"), Some("--query"));
}

#[test]
fn errors_defaults_to_one_hour_window() {
    assert_eq!(defaults_for("errors").since, Some("1h"));
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p cortex --lib actions::tests::tail_binds_positional_to_host_and_defaults_limit`
Expected: FAIL — `positional_for`/`defaults_for` undefined.

- [ ] **Step 4: Add fields, macro support, accessors, and populate**

Add to `struct ActionSpec`:

```rust
    /// Canonical flag a bare positional argument binds to (None = no positional).
    pub positional: Option<&'static str>,
    /// Zero-flag defaults.
    pub defaults: Defaults,
```

Extend the `action_spec!` full-form arm to accept `positional:` and `defaults:`
(default them to `None` / `Defaults::default()` in the short form, mirroring how
Plan 2 added `flags`/`examples`). Populate the relevant actions:

```rust
    // search: positional → query; no forced window.
    positional: Some("--query"), defaults: Defaults { limit: Some(50), since: None },
    // tail: positional → host; n=50.
    positional: Some("--host"), defaults: Defaults { limit: Some(50), since: None },
    // errors: last hour by default.
    positional: None, defaults: Defaults { limit: None, since: Some("1h") },
    // host_state: positional → host.
    positional: Some("--host"), defaults: Defaults::default(),
```

Add accessors:

```rust
pub(super) fn positional_for(action: &str) -> Option<&'static str> {
    ACTION_SPECS.iter().find(|s| s.name == action).and_then(|s| s.positional)
}
pub(super) fn defaults_for(action: &str) -> Defaults {
    ACTION_SPECS.iter().find(|s| s.name == action).map(|s| s.defaults).unwrap_or_default()
}
```

Expose CLI-facing re-exports next to Plan 2's `cli_flags_for` facade.

- [ ] **Step 5: Run tests**

Run: `cargo test -p cortex --lib actions::tests` → PASS.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/action_flags.rs src/mcp/actions.rs src/mcp/actions_tests.rs
git commit -m "feat(mcp): registry metadata for positionals + zero-flag defaults"
```

---

## Task 2: Shared positional-binding + defaults helpers

**Files:**
- Create: `src/cli/argdefaults.rs`, `src/cli/argdefaults_tests.rs`
- Modify: `src/cli.rs` (register module)

- [ ] **Step 1: Write the failing test**

Create `src/cli/argdefaults_tests.rs`:

```rust
use super::*;

#[test]
fn bind_positional_returns_value_for_action_with_positional() {
    // tail binds a bare positional to --host.
    let bound = positional_value("tail", &["devhost".to_string()]).unwrap();
    assert_eq!(bound.as_deref(), Some("devhost"));
}

#[test]
fn bind_positional_errors_when_action_takes_none() {
    // hosts takes no positional; a stray arg is an error.
    let err = positional_value("hosts", &["oops".to_string()]).unwrap_err().to_string();
    assert!(err.contains("unexpected"), "{err}");
}

#[test]
fn apply_default_limit_only_when_unset() {
    assert_eq!(effective_limit("tail", None), Some(50));      // default applied
    assert_eq!(effective_limit("tail", Some(5)), Some(5));    // user wins
    assert_eq!(effective_limit("status", None), None);        // no default
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib argdefaults::tests`
Expected: FAIL — module undefined.

- [ ] **Step 3: Implement the helpers**

Create `src/cli/argdefaults.rs`:

```rust
//! Declarative positional binding + zero-flag defaults, driven by ACTION_SPECS.

use anyhow::{Result, bail};

/// Bind the collected positional tokens for `action` to its positional flag's
/// value. Returns Ok(None) if the action has a positional but none was given;
/// errors if positionals were given but the action accepts none, or if more
/// than one was given.
pub(crate) fn positional_value(action: &str, positionals: &[String]) -> Result<Option<String>> {
    let accepts = crate::cli::registry_positional(action).is_some();
    match (accepts, positionals.len()) {
        (false, 0) => Ok(None),
        (false, _) => bail!("unexpected argument '{}'; this command takes no positional", positionals[0]),
        (true, 0) => Ok(None),
        (true, 1) => Ok(Some(positionals[0].clone())),
        (true, _) => bail!("expected at most one positional argument, got {}", positionals.len()),
    }
}

/// The effective `--limit`: the user's value if set, else the action default.
pub(crate) fn effective_limit(action: &str, user: Option<u32>) -> Option<u32> {
    user.or_else(|| crate::cli::registry_defaults(action).limit)
}

/// The effective `--since`: the user's value if set, else the action default
/// (already an absolute RFC3339 string via the time parser).
pub(crate) fn effective_since(action: &str, user: Option<String>) -> Result<Option<String>> {
    if let Some(v) = user {
        return Ok(Some(v));
    }
    match crate::cli::registry_defaults(action).since {
        Some(rel) => Ok(Some(crate::cli::timearg::parse_time_arg(rel, chrono::Utc::now())?)),
        None => Ok(None),
    }
}
```

Add `registry_positional` / `registry_defaults` to the CLI facade (next to Plan 2's
`registry_flags`). Register `pub(crate) mod argdefaults;` in `src/cli.rs` and add the
sidecar hook in `argdefaults.rs`.

- [ ] **Step 4: Run tests + commit**

Run: `cargo test -p cortex --lib argdefaults` → PASS.

```bash
git add src/cli/argdefaults.rs src/cli/argdefaults_tests.rs src/cli.rs
git commit -m "feat(cli): declarative positional + defaults helpers"
```

---

## Task 3: Apply positionals + defaults in `tail`

**Files:**
- Modify: `src/cli/parse_logs.rs::parse_tail`
- Modify: `src/cli/parse_logs_tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn tail_positional_sets_host_and_default_limit() {
    let cmd = parse_tail(&["devhost".into()]).unwrap();
    let CliCommand::Tail(args) = cmd else { panic!("expected Tail") };
    assert_eq!(args.hostname.as_deref(), Some("devhost"));
    assert_eq!(args.limit, Some(50)); // default applied when n/--limit omitted
}

#[test]
fn tail_explicit_limit_overrides_default() {
    let cmd = parse_tail(&["devhost".into(), "-n".into(), "10".into()]).unwrap();
    let CliCommand::Tail(args) = cmd else { panic!("expected Tail") };
    assert_eq!(args.limit, Some(10));
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib parse_logs::tests::tail_positional_sets_host_and_default_limit`
Expected: FAIL — bare `devhost` is currently an unknown-arg error / no default limit.

- [ ] **Step 3: Implement in `parse_tail`**

Collect non-flag tokens into a `positionals: Vec<String>` during the parse loop
(mirroring how `parse_search` already collects its `query` vec). After the loop:

```rust
use super::argdefaults::{positional_value, effective_limit};

if let Some(host) = positional_value("tail", &positionals)? {
    parsed.hostname = Some(host);
}
parsed.limit = effective_limit("tail", parsed.limit);
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p cortex --lib parse_logs::tests::tail` → PASS (both cases).

- [ ] **Step 5: Commit**

```bash
git add src/cli/parse_logs.rs src/cli/parse_logs_tests.rs
git commit -m "feat(cli): tail accepts a bare hostname positional + default limit"
```

---

## Task 4: Apply positionals/defaults to `search`, `errors`, `host-state`

**Files:**
- Modify: `src/cli/parse_logs.rs` (`parse_search`, `parse_errors`),
  `src/cli/commands/host_state.rs` (`parse_host_state`)
- Modify: their sidecar tests

- [ ] **Step 1: Write the failing tests**

```rust
// parse_logs_tests.rs
#[test]
fn search_applies_default_limit() {
    let cmd = parse_search(&["oom".into()]).unwrap();
    let CliCommand::Search(args) = cmd else { panic!() };
    assert_eq!(args.limit, Some(50));
}

#[test]
fn errors_defaults_to_one_hour_window() {
    let cmd = parse_errors(&[]).unwrap();
    let CliCommand::Errors(args) = cmd else { panic!() };
    let from = args.from.expect("default since applied");
    assert!(from.ends_with("+00:00")); // absolute RFC3339 from the 1h default
}
```

```rust
// commands/host_state tests
#[test]
fn host_state_positional_sets_host() {
    let cmd = parse_host_state(&["devhost".into()]).unwrap();
    // assert the resolved host field equals "devhost" (match the HostStateArgs shape)
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib parse_logs::tests::search_applies_default_limit parse_logs::tests::errors_defaults_to_one_hour_window`
Expected: FAIL.

- [ ] **Step 3: Implement**

- `parse_search`: after collecting `query`, apply `parsed.limit = effective_limit("search", parsed.limit);`
  (the positional → query binding already exists; keep it).
- `parse_errors`: after the loop, `parsed.from = effective_since("errors", parsed.from)?;`
- `parse_host_state`: collect a positional and bind via `positional_value("host_state", &positionals)?`
  into the host field (note the MCP action name is `host_state` with an underscore).

- [ ] **Step 4: Run the suites**

Run: `cargo test -p cortex --lib parse_logs` → PASS.
Run: `cargo test -p cortex --lib host_state` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli/parse_logs.rs src/cli/parse_logs_tests.rs src/cli/commands/host_state.rs
git commit -m "feat(cli): positionals + defaults for search/errors/host-state"
```

---

## Task 5: Help examples reflect the flagless common case

**Files:**
- Modify: `src/mcp/actions.rs` examples (tighten to flagless forms)
- Modify: `scripts/smoke-test.sh`

- [ ] **Step 1: Update examples to the flagless forms**

In the registry `examples` for these actions, lead with the flagless case:

```
search:     cortex search "oom killer"
tail:       cortex tail devhost
errors:     cortex errors
host_state: cortex host-state devhost
```

(Keep one flagged example each to show the knobs exist.)

- [ ] **Step 2: Add smoke cases for the positional forms**

In `scripts/smoke-test.sh`:

```bash
run_case "tail positional host" cortex tail devhost -n 1
run_case "errors default window" cortex errors
```

- [ ] **Step 3: Run help + smoke**

Run: `cargo test -p cortex --lib help` → PASS (update snapshots if present).
Run: `bash scripts/smoke-test.sh` (server up) → PASS.

- [ ] **Step 4: Commit**

```bash
git add src/mcp/actions.rs scripts/smoke-test.sh
git commit -m "docs(cli): examples + smoke for flagless positional/default forms"
```

---

## Task 6: Final verification

- [ ] **Step 1: Gates**

Run: `cargo test -p cortex` → PASS.
Run: `cargo clippy --all-targets -- -D warnings` → clean.
Run: `cargo fmt --check` → clean.

- [ ] **Step 2: Manual common-case sweep (server up)**

```bash
cortex tail devhost            # bare host, n=50
cortex search "oom killer"    # bare query, limit 50
cortex errors                 # last hour
cortex host-state devhost      # bare host
```

- [ ] **Step 3: Commit cleanup**

```bash
git add -A && git commit -m "chore(cli): clippy/fmt cleanup for defaults + positionals"
```

---

## Self-Review

**Spec coverage:** Component 4 (smart defaults + positionals) → Tasks 1–5: registry
metadata (Task 1), shared declarative helpers (Task 2), applied to tail/search/errors/
host-state (Tasks 3–4), examples + smoke (Task 5). No part of Component 4 is left
implicit.

**Placeholder scan:** All helper code is complete. Task 4's `host_state` test leaves
the exact field assertion to match `HostStateArgs` shape (`rg -n "struct HostStateArgs"
src/cli/commands/host_state.rs`) — the only lookup, because that struct wasn't read
during planning; the binding call and action name are concrete.

**Type consistency:** `Defaults{limit,since}`, `positional_for`/`defaults_for`,
`positional_value(&str,&[String]) -> Result<Option<String>>`,
`effective_limit(&str,Option<u32>) -> Option<u32>`,
`effective_since(&str,Option<String>) -> Result<Option<String>>` — consistent across
tasks and aligned with Plan 1 (`parse_time_arg`) and Plan 2 (registry facade).

**Dependency note:** Requires Plan 1 (time parser) and Plan 2 (registry flag metadata
+ CLI facade). Execute in order 1 → 2 → 3.
