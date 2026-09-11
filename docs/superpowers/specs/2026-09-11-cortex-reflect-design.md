# `cortex reflect`: one-shot skill / MCP / hook reflection

Date: 2026-09-11
Status: approved design, revised after engineering review (2026-09-11)

## Goal

Give cortex a single local command that answers "how are my skills, MCP
tools, and hooks behaving, and what should I change?" without the homelab
server, the syslog receiver, or any tokens. It supports two modes:

- **Report + LLM** (default): deterministic incident report plus LLM
  assessments of the highest-severity incidents.
- **Report only** (`--no-llm`): deterministic incident report, no LLM
  binary required.

## Non-goals

- No new detectors, scoring, or prompts. `reflect` orchestrates existing,
  tested code only.
- No `abuse` kind. Kinds are exactly `skill`, `mcp`, `hook`.
- No built-in tool (Bash/Edit/Read) incidents and no subagent kind.
- No daemon or watcher mode, no MCP or REST exposure. `reflect` is a
  local-only CLI command, like `assess`.
- No `--out` flag. The report goes to stdout; progress and warnings go to
  stderr, so `cortex reflect > report.md` works.
- No crate split, cargo feature, or schema fork.

Tracked follow-ups (not in this project): fair share of top-N slots across
kinds, set-based anchor queries in incident search, and an index scan
budget with live progress.

## Command surface

```
cortex reflect [--since 7d] [--until T] [--project P] [--tool claude|codex|gemini]
               [--kinds skill,mcp,hook] [--no-llm] [--max-assess 5]
               [--no-index] [--db PATH] [--json]
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--since` / `--until` | last 7 days / run start | Detection window, same syntax as `assess` |
| `--project` | all | Filter by AI project |
| `--tool` | all | Filter by AI tool (claude, codex, gemini) |
| `--kinds` | `skill,mcp,hook` | Comma-separated subset; any other value is an argument error |
| `--no-llm` | off | Report-only mode |
| `--max-assess` | 5 | How many top incidents (across all kinds) get a detailed section; the LLM runs on these unless `--no-llm`. `0` means summary and table only |
| `--no-index` | off | Skip indexing; detect over what the DB already holds |
| `--db` | see below | SQLite file to use |
| `--json` | off | Emit the report as JSON instead of Markdown |

`--http`, `--server`, `--token`, and `CORTEX_USE_HTTP=1` are rejected:
`reflect` always runs locally.

### Database path resolution

1. `--db PATH`
2. `CORTEX_DB_PATH` when set and non-empty. `reflect` prints a stderr
   warning naming the path, because it may be a live server database.
3. `~/.cortex/reflect.db`. When `~/.cortex` does not exist, `reflect`
   creates it owner-only (0700).

After the pool opens, the database file is set owner-only (0600) on Unix.
The file uses the normal migration ladder; unrelated tables stay empty.

## Pipeline

1. **Setup.** Resolve the DB path, load config via the stdio loader,
   override `storage.db_path`, and build the query-only runtime with the
   existing SQLite-busy retry. No listener is bound and no auth config is
   validated. Pin `until` to the run start time when the user gave none,
   so every later step sees the same window.
2. **Index.** Unless `--no-index`, index the default transcript roots,
   passing `since` as the file-age filter so only transcripts modified in
   the window are read. Print a progress line before and a counts line
   after. Checkpoints keep repeat runs incremental.
3. **Detect.** For each selected kind, call the existing list service with
   the window and filters. The list service returns at most 100 incidents
   per kind; `reflect` records the kind's `total_incidents` and whether the
   candidate window or the list was truncated. All three kinds share one
   scoring formula shape, so incidents merge into one list sorted by score
   descending, then most recent, then incident id. The report states that
   scores are heuristic.
4. **Assess.** Take the top N (`--max-assess`). For each, run the kind's
   investigate service once, with the incident id plus the incident's
   exact target (skill and plugin, MCP server and tool, or hook event and
   name) and the same window and filters. The target filters are exact
   matches on the grouping key, so they shrink the scan without changing
   the incident id. The deterministic findings come from that evidence.
   Unless `--no-llm`, the evidence goes to the kind's existing per-evidence
   LLM helper, with every `LlmRunner` guard in force. Assessments run one
   at a time.
5. **Report.** Build one `ReflectReport`, then render it as Markdown or
   JSON on stdout.

The three assess requests also gain an optional `incident_id`, exposed as
`--incident-id` on `cortex assess skill|mcp|hooks`, so the report can print
an exact command to assess any unassessed incident later.

## Report

`ReflectReport` fields:

- `since`, `until`, `project`, `tool`, `kinds`, `db_path`
- `mode`: `report_only` or `report_and_llm`, plus `llm_fallback_reason`
  when the preflight downgraded the run
- `index`: discovered files, new records, duplicates skipped, parse errors,
  or `null` when skipped
- `summary`: per kind, listed and total incident counts, counts by severity
  label, and `truncated` when the list or candidate window was capped
- `assessed`: per detailed incident: kind, id, target, severity, score,
  signals, the LLM assessment or `null`, the deterministic findings, and a
  `failure` reason when the assessment could not run
- `unassessed`: remaining incidents with the exact
  `cortex assess <kind> --incident-id ... --since ... --until ...` command

Markdown: title, window, filters, database, mode, index line, a fallback
note when present, a summary table with a truncation warning and a
"narrow --since" hint when capped, one H2 section per detailed incident,
then an "Other incidents" table. LLM headings are demoted two levels
outside code fences. Transcript-derived single-line fields have control
characters replaced with spaces; the LLM body keeps newlines and tabs only.
Table cells escape `|` and backticks.

## Error handling

| Situation | Behavior |
|-----------|----------|
| Bad arguments, unknown kind, unusable DB path | Fail before any work, non-zero exit |
| Unreadable or malformed transcript files | Counted as parse errors; run continues |
| LLM backend unresolvable, or its program not found (checked as the exact program string, no whitespace splitting) | Whole run downgrades to report-only; `llm_fallback_reason` set; stderr warning names the fix |
| LLM globally disabled | First attempt reports it; the whole run downgrades to report-only |
| LLM action disabled, or circuit open, for one kind | No more LLM calls for that kind; its incidents show findings plus the reason; other kinds continue |
| One assessment fails (timeout, rate limit, backend error) | That incident keeps its findings plus the reason; the run continues |
| Incident no longer resolves (DB changed during the run) | That incident is marked "incident changed during run"; the run continues |
| No incidents in window | Report says so; exit 0 |
| DB locked when opening | Existing query-only retry (3 attempts), then fail naming the path |

Exit code is 0 whenever a report is produced, including partial LLM
failure.

## Code layout

Sibling `foo.rs` modules, no `mod.rs`, sidecar `*_tests.rs`, every file
under 500 lines.

| File | Purpose |
|------|---------|
| `src/app/models/reflect.rs` | Request, incident, summary, and report types; ranking; summary |
| `src/app/services/reflect.rs` | Pipeline: index, detect, rank, assess |
| `src/app/services/reflect_llm.rs` | Pure LLM-failure classifier and program lookup |
| `src/app/reflect_report.rs` | Markdown renderer |
| `src/cli/args/reflect.rs`, `src/cli/parse/reflect.rs` | Flags |
| `src/cli/dispatch_reflect.rs` | Dispatch and DB path resolution |
| `src/runtime.rs` | Query-only runtime from a prepared config, with retry |
| `src/surfaces.rs` | `local_cli!("reflect", Sessions, Canonical)` |

The per-evidence LLM helpers in the three assessment services become
visible to sibling service modules. They are otherwise unchanged.

Docs: a README subsection, a runbook note, a `CLAUDE.md` Commands entry,
and a `just reflect` recipe.

## Testing

Unit tests:

- Argument parsing: defaults, every flag, unknown and empty kinds,
  duplicate kinds, unknown options.
- DB path resolution order.
- Merged ranking with tie-breaks; per-kind summary with totals and
  truncation.
- Pipeline in report-only mode over seeded skill, MCP, and hook incidents:
  ranking across kinds, the `--max-assess` cap, kind filtering, empty DB,
  and a drifted incident id that becomes a per-incident failure.
- LLM failure classification, and the program lookup.
- Markdown rendering: report-only, LLM, fallback note, failures,
  truncation warning, empty report, heading demotion that skips code
  fences, and control-character stripping.

End-to-end binary tests with Claude transcript fixtures in a temporary
HOME:

- `--no-llm --json` finds the skill incident, creates
  `~/.cortex/reflect.db` owner-only, and a second run indexes nothing new.
- An LLM backend program that exits with failure keeps the report: exit 0
  and a `failure` reason on the assessed incident.
- A missing LLM backend program downgrades the run with
  `llm_fallback_reason`.
- `--http` is rejected.
