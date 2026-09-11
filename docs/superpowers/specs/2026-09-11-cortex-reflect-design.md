# `cortex reflect` — one-shot skill / MCP / hook reflection

Date: 2026-09-11
Status: approved design, pending implementation plan

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
- No built-in tool (Bash/Edit/Read) incidents, no subagent kind. Built-in
  tool incidents may land later as a separate project.
- No daemon or watcher mode, no MCP or REST exposure. `reflect` is a
  local-only CLI command, like `assess`.
- No crate split, cargo feature, or schema fork.

## Command surface

```
cortex reflect [--since 7d] [--until T] [--project P] [--tool claude|codex|gemini]
               [--kinds skill,mcp,hook] [--no-llm] [--max-assess 5]
               [--no-index] [--db PATH] [--json] [--out FILE]
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--since` / `--until` | last 7 days / now | Detection window, same syntax as `assess` |
| `--project` | all | Filter by AI project |
| `--tool` | all | Filter by AI tool (claude, codex, gemini) |
| `--kinds` | `skill,mcp,hook` | Comma-separated subset; any other value is an argument error |
| `--no-llm` | off | Report-only mode |
| `--max-assess` | 5 | How many top incidents (across all kinds) get a detailed section; the LLM runs on these unless `--no-llm`. `0` means summary and table only, no LLM calls |
| `--no-index` | off | Skip indexing; detect over what the DB already holds |
| `--db` | see below | SQLite file to use |
| `--json` | off | Emit the report as JSON instead of Markdown |
| `--out` | stdout | Write the report to a file |

`--http`, `--server`, `--token`, and `CORTEX_USE_HTTP=1` are rejected:
`reflect` always runs locally.

### Database path resolution

1. `--db PATH`
2. `CORTEX_DB_PATH` when set
3. `~/.cortex/reflect.db` (parent directory created if missing)

The file is initialised with the normal migration ladder. Tables unrelated
to reflection are created empty. `reflect` never writes to the homelab
server database unless the user points `--db` or `CORTEX_DB_PATH` at it.

## Pipeline

1. **Setup.** Resolve the DB path, load config via the stdio/query-only
   loader, override `storage.db_path`, and build the query-only runtime
   (`RuntimeCore::query_only`). No listener is bound and no auth config is
   validated.
2. **Index.** Unless `--no-index`, run the transcript scanner over the
   default transcript roots (`scanner::index_roots_with_options` with
   `scanner::providers::paths::transcript_roots()`). Checkpoints make repeat runs incremental. Skill,
   MCP, and hook events are extracted in the same pass.
3. **Detect.** For each selected kind, call the existing service
   investigate method (`investigate_ai_skill_incidents`,
   `investigate_ai_mcp_incidents`, `investigate_ai_hook_incidents`) with
   the window, project, and tool filters. All three use the same scoring
   formula, so incidents merge into one list sorted by score descending,
   then by most recent event, then by incident id for a stable order.
4. **Assess.** Take the top N incidents (N = `--max-assess`) from the
   merged list. Unless `--no-llm`, run each through its kind's existing
   guarded assessment path (`run_skill_assessment_with_delta`,
   `run_mcp_assessment_with_delta`, `run_hook_assessment_with_delta`),
   targeted at that single incident. All `LlmRunner` guards (kill switch,
   concurrency, rate limits, circuit breaker, timeout, size caps, audit
   rows) apply unchanged. Assessments run one at a time.
5. **Report.** Build one `ReflectReport` value, then render it as Markdown
   or JSON.

None of `SkillAssessRequest`, `McpAssessRequest`, or `HookAssessRequest`
can target a single incident today. The implementation adds an optional
`incident_id: Option<String>` to each, matching `AbuseAssessRequest`.
When set, the service assesses only that incident from the investigate
result. That is the only change allowed in existing service code, and it
also gives `cortex assess skill|mcp|hooks` an `--incident-id` flag for
free, which the report's "assess this" commands use.

## Report

`ReflectReport` fields:

- `window` (since, until), `filters` (project, tool, kinds), `db_path`
- `mode`: `report_only` or `report_and_llm`, plus `llm_fallback_reason`
  when the run downgraded to report-only
- `index`: files scanned, new records, parse-error count, or `skipped`
- `summary`: incident counts by kind and severity label
- `assessed`: one entry per assessed incident: kind, incident id, target
  (skill / server+tool / hook), severity, score, and either the LLM
  assessment Markdown or the deterministic findings plus a failure reason
- `unassessed`: remaining incidents with kind, id, target, severity,
  score, and the exact `cortex assess <kind> ...` command to assess it

Markdown layout: title with window, summary table, one H2 section per
assessed incident, then an "Other incidents" table. In `--no-llm` mode the
detailed sections show deterministic findings instead of LLM output, so
the report has the same shape in both modes.

LLM output is not streamed in `reflect`. Progress lines ("indexing…",
"assessing 2/5: skill foo…") go to stderr so stdout stays a clean report.

## Error handling

| Situation | Behavior |
|-----------|----------|
| Bad arguments, unknown kind, unusable DB path | Fail before any work, non-zero exit |
| Unreadable or malformed transcript files | Recorded as parse errors by the scanner; count appears in the report; run continues |
| LLM disabled, backend binary missing, or backend fails preflight | Whole run downgrades to report-only; `llm_fallback_reason` set; warning on stderr naming the fix (`CORTEX_LLM`, `CORTEX_LLM_ENABLED`, install codex/gemini) |
| A single assessment fails (timeout, rate limit, circuit open, backend error) | That incident shows deterministic findings plus the reason; remaining assessments continue |
| Circuit breaker opens mid-run | Remaining incidents are reported with the same reason without further LLM calls |
| No incidents in window | Report says so; exit 0 |
| DB locked by another writer | Existing query-only retry (3 attempts); then fail with the path in the error |

Exit code is 0 whenever a report is produced, including partial LLM
failure.

## Code layout

All new files follow the repo conventions: sibling `foo.rs` modules, no
`mod.rs`, sidecar `*_tests.rs`, each file under the 500-line module limit.

| File | Purpose |
|------|---------|
| `src/cli/args/reflect.rs` | `ReflectArgs` |
| `src/cli/parse/reflect.rs` | Flag parsing and validation |
| `src/cli/dispatch_reflect.rs` | Local-only guard, DB path resolution, runtime construction, output writing |
| `src/app/services/reflect.rs` | Pipeline: index, detect, rank, assess |
| `src/app/models/reflect.rs` | `ReflectRequest`, `ReflectReport`, and related types |
| `src/app/reflect_report.rs` | Markdown renderer |
| `src/surfaces.rs` | `local_cli!("reflect", Sessions, Canonical)` |

Docs: README section "Skill, MCP, and hook reflection", a note in
`docs/runbooks/skill-reflection.md`, a `CLAUDE.md` Commands entry, and a
`just reflect` recipe.

## Testing

Unit tests (sidecar files):

- Argument parsing: defaults, every flag, unknown kind, `--max-assess 0`,
  HTTP flags rejected.
- DB path resolution order.
- Merged ranking across kinds, including tie-breaks.
- `--max-assess` cap across kinds.
- Fallbacks with a stub LLM runner: disabled backend downgrades the run;
  one failed assessment keeps the others; circuit-open short-circuits the
  rest.
- Markdown rendering for report-only, mixed, and empty reports.

Integration test: index small Claude and Codex transcript fixtures (with
a skill load followed by a user correction, an MCP call that errors twice,
and a failed hook) into a temp DB, run `reflect --no-llm --json`, and
assert the incident kinds, ranking, and report shape. A second run asserts
indexing is incremental (zero new records).
