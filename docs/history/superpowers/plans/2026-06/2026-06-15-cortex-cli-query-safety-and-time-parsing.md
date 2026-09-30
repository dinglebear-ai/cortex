# Cortex CLI: Query Safety + Unified Time Parsing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `cortex` searches forgiving (literal `--grep` + fix-it errors for the FTS5 hyphen trap) and let every time flag accept relative input (`1h`, `2d`, `yesterday`) as well as absolute timestamps.

**Architecture:** Two self-contained, pure-ish additions that carry no cross-cutting rename risk. (1) A new pure time-parser module normalizes user time strings to RFC3339 at CLI parse time. (2) A query-lint helper detects the common FTS5 foot-guns and returns fix-it messages, plus a `--grep` literal mode that wraps input as an FTS5 quoted phrase so hyphens/operators are treated literally. This is Plan 1 of 3 from the CLI-ergonomics spec; the registry/completion/rename work (Plan 2) and defaults/positionals (Plan 3) follow.

**Tech Stack:** Rust (edition 2024), anyhow, chrono (already a dep via timestamps), existing `FlagCursor` parse helpers, `ServiceError::InvalidInput`, sidecar `*_tests.rs` unit tests run with `cargo test`.

**Spec:** `docs/superpowers/specs/2026-06-15-cortex-cli-ergonomics-design.md` (Components 3 and 6).

---

## File Structure

- **Create** `src/cli/timearg.rs` — pure time-argument normalizer (`parse_time_arg`). One responsibility: turn a user string into an RFC3339 string, given an injected `now`.
- **Create** `src/cli/timearg_tests.rs` — sidecar unit tests for the parser.
- **Modify** `src/cli.rs` — register `pub(crate) mod timearg;`.
- **Modify** `src/cli/parse_logs.rs` — route the existing `--from/--to/--received-since/--received-until` values through `parse_time_arg` so relative input is accepted. (Flag *names* are unchanged here; the rename to `--since/--until` is Plan 2.)
- **Modify** `src/db/queries.rs` — add `lint_fts_query` (fix-it detection) and call it from `validate_fts_query`; add `fts_phrase_literal` to wrap `--grep` input as a safe FTS5 phrase.
- **Modify** `src/db/queries_tests.rs` — sidecar tests for the lint + phrase helpers.
- **Modify** the search args struct + `parse_search` (in `src/cli/args*.rs` and `src/cli/parse_logs.rs`) — add a `grep: Option<String>` field, parse `--grep`, and make it mutually exclusive with `--query`.

> Note on `now` injection: production callers pass `chrono::Utc::now()`. Tests pass a fixed `DateTime<Utc>` so the parser is deterministic. Never call `Utc::now()` inside the pure function.

---

## Task 1: Time-parser module skeleton + relative durations

**Files:**
- Create: `src/cli/timearg.rs`
- Create: `src/cli/timearg_tests.rs`
- Modify: `src/cli.rs` (add module registration)

- [ ] **Step 1: Register the module**

In `src/cli.rs`, add alongside the other `mod` lines:

```rust
pub(crate) mod timearg;
```

- [ ] **Step 2: Write the failing test for relative durations**

Create `src/cli/timearg_tests.rs`:

```rust
use super::*;
use chrono::{TimeZone, Utc};

fn fixed_now() -> chrono::DateTime<Utc> {
    // 2026-06-15T12:00:00Z
    Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
}

#[test]
fn parses_relative_durations_back_from_now() {
    let now = fixed_now();
    assert_eq!(parse_time_arg("1h", now).unwrap(), "2026-06-15T11:00:00+00:00");
    assert_eq!(parse_time_arg("30m", now).unwrap(), "2026-06-15T11:30:00+00:00");
    assert_eq!(parse_time_arg("2d", now).unwrap(), "2026-06-13T12:00:00+00:00");
    assert_eq!(parse_time_arg("90s", now).unwrap(), "2026-06-15T11:58:30+00:00");
}

#[test]
fn rejects_unknown_relative_unit() {
    let now = fixed_now();
    let err = parse_time_arg("5w", now).unwrap_err().to_string();
    assert!(err.contains("time"), "error should mention time: {err}");
}
```

- [ ] **Step 3: Write the module to make it compile and the test fail meaningfully**

Create `src/cli/timearg.rs`:

```rust
//! Normalize user-supplied time arguments to RFC3339.
//!
//! Accepts relative durations (`1h`, `30m`, `2d`, `90s`), the keywords
//! `now`/`today`/`yesterday`, and absolute timestamps (RFC3339, `YYYY-MM-DD`,
//! `YYYY-MM-DD HH:MM`). `now` is injected for deterministic testing — never
//! read the clock inside this module.

use anyhow::{Result, bail};
use chrono::{DateTime, Duration, Utc};

/// Convert a user time string into an RFC3339 timestamp string.
pub(crate) fn parse_time_arg(input: &str, now: DateTime<Utc>) -> Result<String> {
    let s = input.trim();
    if s.is_empty() {
        bail!("empty time value");
    }
    if let Some(dt) = parse_relative(s, now)? {
        return Ok(dt.to_rfc3339());
    }
    bail!("unrecognized time value '{s}'; use e.g. 1h, 30m, 2d, yesterday, or an RFC3339 timestamp")
}

/// Parse a relative duration or keyword. Returns `Ok(None)` if `s` is not a
/// relative form (so the caller can try absolute parsing).
fn parse_relative(s: &str, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    let (value, unit) = s.split_at(s.len() - 1);
    let unit_char = s.chars().last().unwrap();
    if let Ok(n) = value.parse::<i64>() {
        let dur = match unit_char {
            's' => Duration::seconds(n),
            'm' => Duration::minutes(n),
            'h' => Duration::hours(n),
            'd' => Duration::days(n),
            _ => bail!("unknown time unit '{unit}'; use s, m, h, or d (e.g. 90s, 2d)"),
        };
        return Ok(Some(now - dur));
    }
    Ok(None)
}
```

- [ ] **Step 4: Add the sidecar test hook to the module**

Append to the end of `src/cli/timearg.rs`:

```rust
#[cfg(test)]
#[path = "timearg_tests.rs"]
mod tests;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p cortex --lib timearg`
Expected: PASS for `parses_relative_durations_back_from_now` and `rejects_unknown_relative_unit`.

- [ ] **Step 6: Commit**

```bash
git add src/cli/timearg.rs src/cli/timearg_tests.rs src/cli.rs
git commit -m "feat(cli): add relative time-argument parser (1h, 30m, 2d, 90s)"
```

---

## Task 2: Keywords (now / today / yesterday)

**Files:**
- Modify: `src/cli/timearg.rs`
- Modify: `src/cli/timearg_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/cli/timearg_tests.rs`:

```rust
#[test]
fn parses_keywords() {
    let now = fixed_now();
    assert_eq!(parse_time_arg("now", now).unwrap(), "2026-06-15T12:00:00+00:00");
    // `today` = midnight UTC of the current day
    assert_eq!(parse_time_arg("today", now).unwrap(), "2026-06-15T00:00:00+00:00");
    // `yesterday` = midnight UTC of the previous day
    assert_eq!(parse_time_arg("yesterday", now).unwrap(), "2026-06-14T00:00:00+00:00");
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p cortex --lib timearg::tests::parses_keywords`
Expected: FAIL — `unrecognized time value 'now'`.

- [ ] **Step 3: Implement keyword handling**

In `src/cli/timearg.rs`, add to `parse_time_arg` before the `parse_relative` call:

```rust
    match s.to_ascii_lowercase().as_str() {
        "now" => return Ok(now.to_rfc3339()),
        "today" => return Ok(start_of_day(now, 0).to_rfc3339()),
        "yesterday" => return Ok(start_of_day(now, 1).to_rfc3339()),
        _ => {}
    }
```

Add the helper to the same file:

```rust
use chrono::TimeZone;

/// Midnight UTC, `days_ago` days before `now`.
fn start_of_day(now: DateTime<Utc>, days_ago: i64) -> DateTime<Utc> {
    let d = (now - Duration::days(days_ago)).date_naive();
    Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p cortex --lib timearg`
Expected: PASS (all three test fns).

- [ ] **Step 5: Commit**

```bash
git add src/cli/timearg.rs src/cli/timearg_tests.rs
git commit -m "feat(cli): time parser keywords now/today/yesterday"
```

---

## Task 3: Absolute timestamps (RFC3339, date, date-time)

**Files:**
- Modify: `src/cli/timearg.rs`
- Modify: `src/cli/timearg_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/cli/timearg_tests.rs`:

```rust
#[test]
fn parses_absolute_timestamps() {
    let now = fixed_now();
    // Full RFC3339 passes through (normalized to +00:00).
    assert_eq!(
        parse_time_arg("2026-06-01T08:30:00Z", now).unwrap(),
        "2026-06-01T08:30:00+00:00"
    );
    // Date-only → midnight UTC.
    assert_eq!(parse_time_arg("2026-06-01", now).unwrap(), "2026-06-01T00:00:00+00:00");
    // Date + HH:MM → that minute, UTC.
    assert_eq!(parse_time_arg("2026-06-01 08:30", now).unwrap(), "2026-06-01T08:30:00+00:00");
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p cortex --lib timearg::tests::parses_absolute_timestamps`
Expected: FAIL — `unrecognized time value '2026-06-01T08:30:00Z'`.

- [ ] **Step 3: Implement absolute parsing**

In `src/cli/timearg.rs`, replace the final `bail!` in `parse_time_arg` with absolute attempts:

```rust
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc).to_rfc3339());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).to_rfc3339());
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Ok(Utc.from_utc_datetime(&ndt).to_rfc3339());
    }
    bail!("unrecognized time value '{s}'; use e.g. 1h, 30m, 2d, yesterday, 2026-06-01, or an RFC3339 timestamp")
```

- [ ] **Step 4: Run the full module test suite**

Run: `cargo test -p cortex --lib timearg`
Expected: PASS (relative, keywords, absolute, rejection).

- [ ] **Step 5: Commit**

```bash
git add src/cli/timearg.rs src/cli/timearg_tests.rs
git commit -m "feat(cli): time parser accepts RFC3339, date, and date-time forms"
```

---

## Task 4: Wire the parser into the log time flags

**Files:**
- Modify: `src/cli/parse_logs.rs` (the `parse_search` and `parse_filter` time-flag arms)
- Modify: `src/cli/parse_logs_tests.rs`

> The flags keep their current names (`--from`, `--to`, `--received-since`, `--received-until`) in this plan; renaming to `--since/--until` is Plan 2. Here we only normalize their *values* through `parse_time_arg`.

- [ ] **Step 1: Write the failing test**

Add to `src/cli/parse_logs_tests.rs`:

```rust
#[test]
fn search_normalizes_relative_from() {
    let cmd = parse_search(&["error".into(), "--since".into(), "1h".into()]).unwrap();
    let CliCommand::Search(args) = cmd else { panic!("expected Search") };
    let from = args.from.expect("from set");
    // Relative input is normalized to an absolute RFC3339 timestamp.
    assert!(from.contains('T') && from.ends_with("+00:00"), "got {from}");
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p cortex --lib parse_logs::tests::search_normalizes_relative_from`
Expected: FAIL — `from` is the literal `"1h"`, not an RFC3339 string.

- [ ] **Step 3: Add a normalizing helper and apply it**

In `src/cli/parse_logs.rs`, add near the top (after imports):

```rust
use super::timearg::parse_time_arg;

/// Normalize a user time value (relative or absolute) to RFC3339 at parse time.
fn norm_time(raw: String) -> anyhow::Result<String> {
    parse_time_arg(&raw, chrono::Utc::now())
}
```

Then change each time-flag assignment in `parse_search` (and the equals-form arms) from:

```rust
"--since" => parsed.from = Some(flags.value("--since")?),
```

to:

```rust
"--since" => parsed.from = Some(norm_time(flags.value("--since")?)?),
```

Apply the same wrap to `--to`, `--received-since`, `--received-until` in both the
space-separated and `--flag=value` arms.

- [ ] **Step 4: Run the test**

Run: `cargo test -p cortex --lib parse_logs::tests::search_normalizes_relative_from`
Expected: PASS.

- [ ] **Step 5: Run the broader parse suite to catch regressions**

Run: `cargo test -p cortex --lib parse_logs`
Expected: PASS (existing absolute-timestamp tests still pass — RFC3339 in → RFC3339 out).

- [ ] **Step 6: Commit**

```bash
git add src/cli/parse_logs.rs src/cli/parse_logs_tests.rs
git commit -m "feat(cli): accept relative time values for search/filter time flags"
```

---

## Task 5: FTS5 fix-it lint (the hyphen-NOT trap)

**Files:**
- Modify: `src/db/queries.rs` (add `lint_fts_query`, call from `validate_fts_query`)
- Modify: `src/db/queries_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/db/queries_tests.rs`:

```rust
#[test]
fn lint_flags_leading_hyphen_term() {
    let err = validate_fts_query("smoke-test").unwrap_err().to_string();
    assert!(err.contains("NOT operator"), "should explain hyphen trap: {err}");
    assert!(err.contains("--grep") || err.contains("\"smoke-test\""), "should suggest a fix: {err}");
}

#[test]
fn lint_accepts_quoted_phrase() {
    // Already-quoted hyphenated phrase is valid FTS5 and must pass.
    assert!(validate_fts_query("\"smoke-test\"").is_ok());
}

#[test]
fn lint_accepts_normal_boolean_query() {
    assert!(validate_fts_query("error AND nginx").is_ok());
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib queries::tests::lint_flags_leading_hyphen_term`
Expected: FAIL — `validate_fts_query("smoke-test")` currently returns `Ok(())`.

- [ ] **Step 3: Implement the lint and call it**

In `src/db/queries.rs`, add:

```rust
/// Detect common FTS5 foot-guns and return a fix-it error. Runs before the
/// generic length/term-count checks in `validate_fts_query`.
fn lint_fts_query(query: &str) -> Result<()> {
    // A bare term containing a hyphen (outside quotes) is parsed by FTS5 as
    // `term NOT term`, which surprises users searching for hyphenated words.
    let has_unquoted_hyphen = !query.contains('"')
        && query
            .split_whitespace()
            .any(|t| t.len() > 1 && t.contains('-') && !t.starts_with('-'));
    if has_unquoted_hyphen {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            "hyphen is the FTS5 NOT operator; quote hyphenated terms as a phrase \
             (e.g. \"smoke-test\") or use --grep for literal text"
                .to_string(),
        )));
    }
    // Unbalanced double-quote.
    if query.matches('"').count() % 2 != 0 {
        return Err(anyhow::Error::new(crate::app::ServiceError::InvalidInput(
            "unbalanced quote in search query; wrap phrases in matching double quotes".to_string(),
        )));
    }
    Ok(())
}
```

Then call it at the top of `validate_fts_query`:

```rust
pub fn validate_fts_query(query: &str) -> Result<()> {
    lint_fts_query(query)?;
    // ... existing length + term-count checks unchanged ...
```

- [ ] **Step 4: Run the lint tests**

Run: `cargo test -p cortex --lib queries::tests::lint`
Expected: PASS (leading-hyphen flagged, quoted phrase ok, boolean query ok).

- [ ] **Step 5: Run the full queries suite for regressions**

Run: `cargo test -p cortex --lib queries`
Expected: PASS (no existing valid query newly rejected).

- [ ] **Step 6: Commit**

```bash
git add src/db/queries.rs src/db/queries_tests.rs
git commit -m "feat(db): FTS5 fix-it lint for hyphen-NOT trap and unbalanced quotes"
```

---

## Task 6: `fts_phrase_literal` helper for `--grep`

**Files:**
- Modify: `src/db/queries.rs`
- Modify: `src/db/queries_tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/db/queries_tests.rs`:

```rust
#[test]
fn phrase_literal_wraps_and_escapes() {
    // Hyphenated input becomes a quoted FTS5 phrase (hyphen treated literally).
    assert_eq!(fts_phrase_literal("smoke-test"), "\"smoke-test\"");
    // Embedded double-quotes are doubled per FTS5 string rules.
    assert_eq!(fts_phrase_literal(r#"say "hi""#), "\"say \"\"hi\"\"\"");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p cortex --lib queries::tests::phrase_literal_wraps_and_escapes`
Expected: FAIL — `fts_phrase_literal` is not defined.

- [ ] **Step 3: Implement the helper**

In `src/db/queries.rs`:

```rust
/// Wrap literal user text as a single FTS5 phrase so operators/hyphens are
/// matched literally. Embedded double-quotes are doubled per FTS5 syntax.
pub fn fts_phrase_literal(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p cortex --lib queries::tests::phrase_literal_wraps_and_escapes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/db/queries.rs src/db/queries_tests.rs
git commit -m "feat(db): fts_phrase_literal wraps --grep input as a safe FTS5 phrase"
```

---

## Task 7: `--grep` flag on `search` (mutually exclusive with `--query`)

**Files:**
- Modify: the `SearchArgs` struct (in `src/cli/args.rs` or wherever `SearchArgs` is defined — grep it: `rg "struct SearchArgs" src/cli`)
- Modify: `src/cli/parse_logs.rs` (`parse_search`)
- Modify: `src/cli/parse_logs_tests.rs`

- [ ] **Step 1: Locate the struct**

Run: `rg -n "struct SearchArgs" src/cli`
Add a field to `SearchArgs`:

```rust
/// Literal substring text (FTS5-safe); mutually exclusive with the positional query.
pub grep: Option<String>,
```

(Add `grep: None` to any manual `Default`/constructor if one exists; if it derives
`Default`, nothing else is needed.)

- [ ] **Step 2: Write the failing test**

Add to `src/cli/parse_logs_tests.rs`:

```rust
#[test]
fn search_grep_sets_literal_and_rejects_with_query() {
    let cmd = parse_search(&["--grep".into(), "smoke-test".into()]).unwrap();
    let CliCommand::Search(args) = cmd else { panic!("expected Search") };
    assert_eq!(args.grep.as_deref(), Some("smoke-test"));

    // --grep together with a positional query is an error.
    let err = parse_search(&["error".into(), "--grep".into(), "x".into()])
        .unwrap_err()
        .to_string();
    assert!(err.contains("--grep"), "should explain the conflict: {err}");
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p cortex --lib parse_logs::tests::search_grep_sets_literal_and_rejects_with_query`
Expected: FAIL — `--grep` is an unknown flag.

- [ ] **Step 4: Parse `--grep` and enforce exclusivity**

In `parse_logs.rs::parse_search`, add to the flag match (both space and `=` forms):

```rust
"--grep" => parsed.grep = Some(flags.value("--grep")?),
_ if arg.starts_with("--grep=") => parsed.grep = Some(value_after_equals(arg, "--grep")?),
```

After the parse loop, before returning, add the exclusivity check (the positional
query is collected into the local `query` vec):

```rust
if parsed.grep.is_some() && !query.is_empty() {
    bail!("--grep and a positional query are mutually exclusive; use one or the other");
}
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p cortex --lib parse_logs::tests::search_grep_sets_literal_and_rejects_with_query`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/cli/args.rs src/cli/parse_logs.rs src/cli/parse_logs_tests.rs
git commit -m "feat(cli): add --grep literal-search flag to search (excl. with query)"
```

---

## Task 8: Apply `--grep` in the search execution path

**Files:**
- Modify: wherever `SearchArgs` is turned into a server/db query (grep: `rg -n "SearchArgs|grep|fts_phrase_literal" src/cli/dispatch*.rs src/app`)
- Modify: the corresponding sidecar test file for that dispatch path

- [ ] **Step 1: Find the consumer**

Run: `rg -n "SearchArgs" src/cli src/app`
Identify where `args.query` (the positional FTS5 query) is read and sent to search.

- [ ] **Step 2: Write the failing test**

In the consumer's sidecar test file, add a test asserting that when `grep` is set,
the effective FTS5 query equals `fts_phrase_literal(grep)` and `query` is ignored.
Model it on the existing dispatch tests in that file (match their construction
helpers). Example shape:

```rust
#[test]
fn grep_becomes_quoted_phrase_query() {
    let args = SearchArgs { grep: Some("smoke-test".into()), ..Default::default() };
    let effective = effective_search_query(&args); // helper added in Step 3
    assert_eq!(effective, "\"smoke-test\"");
}
```

- [ ] **Step 3: Run to confirm failure, then implement**

Run the new test; expect FAIL (`effective_search_query` undefined). Add the helper
next to the consumer:

```rust
/// The FTS5 query string to execute: the literal phrase form when `--grep` is
/// set, otherwise the user's raw `--query`/positional.
fn effective_search_query(args: &SearchArgs) -> String {
    match &args.grep {
        Some(text) => crate::db::queries::fts_phrase_literal(text),
        None => args.query.clone().unwrap_or_default(),
    }
}
```

Wire `effective_search_query(&args)` into the call that currently passes the raw
query string to the search service.

- [ ] **Step 4: Run the test**

Run: `cargo test -p cortex --lib <module>::tests::grep_becomes_quoted_phrase_query`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): route --grep through fts_phrase_literal in search execution"
```

---

## Task 9: Help text + smoke coverage

**Files:**
- Modify: `src/cli/help.rs` (search usage example block — `rg -n "search" src/cli/help.rs`)
- Modify: `scripts/smoke-test.sh`

- [ ] **Step 1: Add `--grep` and relative-time examples to search help**

In `src/cli/help.rs`, find the `search` usage/example text and add two lines (match
the surrounding formatting/token helpers exactly):

```
cortex search "oom killer" --since 1h
cortex search --grep "smoke-test"          # literal, no FTS5 syntax
```

> Note: if help examples are snapshot-tested (`src/cli/help_tests.rs`), update the
> snapshot in the same commit. Run `cargo test -p cortex --lib help` to check.

- [ ] **Step 2: Add a smoke assertion**

In `scripts/smoke-test.sh`, add a case that runs a literal search and expects success:

```bash
run_case "search --grep literal" \
  cortex search --grep "smoke-test" --limit 1
```

(Match the existing `run_case`/helper names in the script — `rg -n "run_case|^run_" scripts/smoke-test.sh`.)

- [ ] **Step 3: Verify build + targeted tests**

Run: `cargo test -p cortex --lib help`
Expected: PASS.
Run: `cargo build`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add src/cli/help.rs scripts/smoke-test.sh
git commit -m "docs(cli): document --grep and relative-time search examples"
```

---

## Task 10: Final verification

- [ ] **Step 1: Full test + lint gates**

Run: `cargo test -p cortex`
Expected: PASS.
Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.
Run: `cargo fmt --check`
Expected: clean.

- [ ] **Step 2: Manual smoke (server running)**

```bash
cortex search --grep "smoke-test" --limit 3        # no FTS5 error
cortex search "error AND nginx" --since 30m --limit 5
cortex search "smoke-test"                          # now returns the fix-it message, not a raw DB error
```

- [ ] **Step 3: Commit any fmt/clippy fixes**

```bash
git add -A
git commit -m "chore(cli): clippy/fmt cleanup for query-safety + time-parsing"
```

---

## Self-Review

**Spec coverage (Components 3 & 6 of the spec):**
- Component 6 (unified time parsing): Tasks 1–4 — relative, keywords, absolute, wired into time flags. ✓
- Component 3 (`--grep` literal): Tasks 6–8. ✓
- Component 3 (FTS5 fix-it errors): Task 5. ✓
- Component 3 (`--query`/`--grep` mutual exclusion): Task 7 Step 4. ✓
- Components 1, 2, 4, 5, 7 (registry metadata, completion, canonical rename, defaults/positionals, discoverability, migration): **out of scope — Plans 2 and 3.**

**Placeholder scan:** Tasks 1–7 and 9–10 contain complete code. Task 8 intentionally
defers the exact call-site to a `rg` lookup because the search-dispatch wiring wasn't
read during planning; the helper code and test shape are concrete, but the worker must
locate the one call site. This is the only investigate-then-wire step.

**Type consistency:** `parse_time_arg(&str, DateTime<Utc>) -> Result<String>`,
`fts_phrase_literal(&str) -> String`, `lint_fts_query(&str) -> Result<()>`,
`SearchArgs.grep: Option<String>`, `effective_search_query(&SearchArgs) -> String`
— names used consistently across tasks.

**Open follow-ups for Plan 2:** rename `--from/--to` → `--since/--until` (the parser
already accepts the values); surface `--grep` and time formats through the new
completion + discoverability help.
