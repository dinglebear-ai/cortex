# Cortex CLI Ergonomics — Guided Deterministic Front Door

**Status:** Design / approved for planning
**Date:** 2026-06-15
**Scope:** Thread B of the "leverage cortex" effort (CLI/UX). Thread A (proactive
data intelligence) is a separate, later design and is explicitly out of scope here.

## Problem

The `cortex` CLI exposes 46 action-dispatched commands (`cortex <action> [flags]`),
each with its own flag set, reachable only by repeatedly consulting `help`. Four
distinct pains, all confirmed as real:

1. **Discoverability** — you know what you want to know, not which action provides it.
2. **Flag recall** — you know the action, not its knobs (`--host` vs `--source`,
   time formats, severity levels).
3. **Query syntax** — FTS5 gotchas (hyphen = NOT operator, phrase quoting for
   hyphenated terms) produce cryptic DB errors.
4. **Workflow chaining** — one command rarely answers the question; you tail, then
   search, then correlate, then context.

There is no *interaction model*, just 46 flat actions addressed by memory.

## Goals / success criteria

- **90% of common tasks done without opening `help`.**
- The common case collapses to `cortex <action> <thing>` (a single positional).
- Tab-completion reveals what exists and fills in **real values** (live hostnames,
  apps, source IPs), not just names.
- Deterministic and scriptable: no LLM in the hot path. (Natural-language access to
  this data already exists via the MCP tool + skills; the CLI's job is to be the
  fast, deterministic tool.)

## Non-goals (explicitly out of scope)

- Natural-language CLI front door (`cortex "errors on devhost last hour"`). NL belongs
  at the agent/MCP layer, not the CLI.
- Interactive TUI/REPL. Possible later; not part of this design.
- High-level workflow verbs (`cortex triage <host>`, `cortex why`). Deferred to
  Thread A, where multi-step intelligence is the point.
- Proactive correlation / digests / anomaly narratives — all Thread A.

## Key architectural principle

Everything is driven from the single authoritative registry, `ACTION_SPECS` in
`src/mcp/actions.rs`. The CLI, completion candidates, help text, and the MCP tool
schema are all generated from it. Adding canonical flag metadata and examples to
`ACTION_SPECS` keeps the CLI, completion, help, MCP surface, and docs from drifting.

## Clean break

Cortex is pre–prod-ready, so this is a **clean rename**, not an alias layer. Canonical
flag/param names replace the old ones across **both** the CLI and the MCP tool
arguments. The migration of the cortex skills, docs, and smoke tests to the new names
is part of this work (see Component 7).

## Components

Each component is independently buildable and testable.

### 1. Dynamic completion engine (centerpiece)

A hidden machine-readable command, `cortex __complete <context>`, emits completion
candidates (TSV: `value\tdescription`) that the zsh completion function consumes.

Two candidate sources:
- **Static** — from `ACTION_SPECS`: action names + descriptions, flag names + types,
  fixed enums (severity levels, `stream=stdout|stderr`, source kinds).
- **Dynamic** — from the DB/server: live hostnames, app names, source IPs, session
  IDs, project paths, via fast bounded read queries.

Behavior:
- Dynamic results cached ~60 s in a tmp file (e.g. `$XDG_RUNTIME_DIR/cortex-complete/`)
  keyed by candidate kind, to avoid hammering the server on every Tab.
- Hard timeout (~150 ms) on the dynamic call; on timeout or unreachable server,
  **degrade silently to static** candidates.
- Installed via `cortex completions zsh` (prints the script; an install hook can drop
  it into the user's fpath). zsh is the only target for v1; bash/fish later.

This single component resolves Discoverability and Flag recall.

### 2. Canonical flag vocabulary (clean rename)

One name per concept, identical across every action and across CLI + MCP:

| Concept | Canonical | Replaces | Notes |
|---|---|---|---|
| host | `--host` (+ positional where obvious) | `--host` | dynamic completion |
| FTS5 query | `--query` (+ positional) | `query` | raw FTS5 |
| literal text | `--grep` | (new) | substring, FTS5-safe (Component 3) |
| result limit | `-n`, `--limit` | `--limit` | |
| min severity | `-s`, `--severity` | `--severity` | enum completion |
| app / program | `--app` | `--app` | |
| source id | `--source` | `--source` | `docker://…` or `IP:port` |
| container | `--container` | `--container` | unchanged |
| stream | `--stream` | `--stream` | `stdout`/`stderr` enum |
| source kind | `--source-kind` | `--source-kind` | enum |
| event-time window | `--since`, `--until` | `--from`, `--to` | unified parser (Component 6) |
| received-time window | `--received-since`, `--received-until` | `--received-since`, `--received-until` | |
| JSON output | `--json` | `--json` | global |

The canonical name and aliases (`-n`, `-s`) live in `ACTION_SPECS` as flag metadata so
completion and help present them consistently. A shared "common selector" flag group
is inherited by every query-bearing action so `--host`/`--since`/`--limit` behave
identically everywhere.

### 3. Query safety (FTS5 trap)

- Add `--grep <text>`: literal/substring matching via escaped `LIKE` (or an FTS5
  phrase wrap), bypassing FTS5 operator parsing entirely. Hyphens and special chars
  "just work."
- For raw `--query`, detect the common foot-guns (a bare leading-hyphen term, unbalanced
  quotes) and return a **fix-it error** — e.g. `hyphen is the FTS5 NOT operator; did
  you mean "smoke-test"? (or use --grep smoke-test)` — instead of surfacing the raw DB
  error.
- `--query` and `--grep` are mutually exclusive; specifying both is a clear error.

### 4. Smart defaults + positionals

- Every action gets sensible zero-flag behavior: `cortex tail` → n=50 across all hosts;
  `cortex errors` → recent window; `cortex search "oom"` → limit 50.
- A positional binds to each action's obvious primary arg, declared in `ACTION_SPECS`:
  - `cortex search "oom killer"` (positional → `--query`)
  - `cortex tail devhost` (positional → `--host`)
  - `cortex host-state devhost` (positional → `--host`)
- Defaults and the positional mapping are metadata on the action spec, not per-command
  special-casing.

### 5. Discoverability surface ("no-help help")

- `cortex` with no args → grouped, Aurora-CLI-tokened action list (existing category
  groups) with a one-line description per action and 2–3 example invocations at the
  bottom. Respects `NO_COLOR` / `--color`.
- `cortex <action>` invoked without a required arg → short usage line + 2–3 **real
  examples**, not a wall of flags. Full flag detail still available via `--help`.
- Examples become a first-class field on each `ACTION_SPEC` entry, so they appear in
  help, the overview, and stay in sync.

### 6. Unified time parsing

A single parser used behind every `--since`/`--until`/`--received-*` flag:
- Relative: `1h`, `30m`, `2d`, `90s`, `yesterday`, `today`.
- Absolute: ISO 8601, `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`.
- Consistent error messages. Removes the "what time format does this one want" pain.

### 7. Skills / docs / smoke-test migration (cost of the clean break)

Because flag/param names change in `ACTION_SPECS` (MCP args included):
- Update the 9 cortex plugin skills' example invocations to canonical names.
- Update `CLAUDE.md`, `docs/mcp/TOOLS.md`, `docs/mcp/SCHEMA.md`, README examples.
- Update `scripts/smoke-test.sh` and `config/mcporter.json` examples.
- Regenerate the MCP tool schema and the action table in `CLAUDE.md`.

## Build order

1. `ACTION_SPECS` gains canonical-flag metadata, positional mapping, defaults, and
   examples fields.
2. `cortex __complete` endpoint (static first, then dynamic + cache + timeout).
3. zsh completion script + `cortex completions zsh`.
4. Query-safety (`--grep` + fix-it errors) and unified time parser — independent,
   can land in parallel.
5. Smart defaults + positionals.
6. Discoverability surface (no-args overview, missing-arg usage).
7. Skills/docs/smoke-test migration; regenerate schema + action table.

## Testing

- **Unit:** `ACTION_SPECS` metadata invariants (every action has a description; every
  positional maps to a real flag; canonical names are unique). Time parser table tests
  (relative + absolute + error cases). Query-safety: `--grep` escaping, FTS5 fix-it
  detection, mutual-exclusion error.
- **Completion:** `cortex __complete` output for representative contexts (action list,
  flag list per action, enum values); dynamic-source fallback-to-static on a forced
  timeout / unreachable server.
- **Smoke:** extend `scripts/smoke-test.sh` to exercise canonical flags and at least
  one positional form per action group.
- **Help/overview:** snapshot tests on the no-args overview and a missing-arg usage,
  with `NO_COLOR` to keep snapshots stable.

## Risks / open questions

- **Dynamic completion latency.** 150 ms budget + 60 s cache should keep Tab snappy;
  if the DB read for source IPs (high-cardinality) is slow, cap candidate counts and
  prefer recently-seen values.
- **Clean-break blast radius.** Renaming MCP args breaks any external caller not in
  this repo. Acceptable pre–prod-ready, but the migration checklist (Component 7) must
  be complete in the same change set so the plugin skills don't silently break.
- **Positional ambiguity.** A few actions have two plausible "primary" args; the
  `ACTION_SPECS` positional mapping must pick one explicitly and the rest stay flags.
