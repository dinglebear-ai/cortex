# Real-Time AI Transcript Ingestion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Make Claude/Codex transcript ingestion real-time without duplicating the existing scanner, checkpoint, dedupe, parse-error, storage-budget, or DB-write logic.

**Architecture:** Add a host-local watcher that observes scanner-owned transcript roots and delegates stable changed files to the existing `SyslogService` scanner path. The watcher is a discovery/scheduling adapter only. Parsing, row identity, checkpoint advancement, storage enforcement, parse-error persistence, and DB writes remain in `scanner`/`SyslogService`. Docker Compose remains the only server deployment path; the watcher is a user-level host helper because it needs access to `$HOME/.claude/projects` and `$HOME/.codex/sessions`.

**Tech Stack:** Rust 1.86, `notify = "8.2.0"`, Tokio, SQLite/WAL via `rusqlite`, existing `SyslogService`, user-level systemd, Docker Compose runtime verification.

**Implementation Status:** Completed in PR #24. Checkboxes below now record the implemented state after lavra research, lavra engineering review, lavra-review, code simplifier passes, pr-review-toolkit review, and GitHub PR comment fixes.

---

## Research and Engineering Review Findings Applied

- Pin `notify` to `8.2.0`; do not use `notify` 9 pre-release APIs such as `EventKindMask`.
- Register watches before the initial scan so writes cannot land between scan and watch readiness.
- Check `event.need_rescan()` before normal create/modify handling and rate-limit full rescans.
- Use bounded/coalesced event delivery. Never store unbounded raw `notify::Event` values.
- Do not drop unstable files; requeue with capped backoff.
- Do not let one bad file, transient metadata error, parse error, or SQLite busy kill the watcher.
- Treat partial trailing JSONL/parse-error results from watcher-triggered indexing as retryable.
- Avoid duplicate root/file policy by exposing scanner-owned helpers for default roots and supported transcript files.
- Add append-offset scanner support using existing checkpoint fields so active files do not get fully rescanned after every append.
- Resolve the exact `syslog` binary and exact live writable DB path during setup; fail on ambiguity.
- Harden the systemd unit with a fixed environment file, `UMask=0077`, `Restart=on-failure`, restart limits, and explicit read/write paths.
- Handle both Ctrl-C and SIGTERM in the watcher.
- Disable the old `syslog-ai-index.timer` during watcher install and verify it is inactive/disabled or absent.
- Verify against Docker Compose specifically; do not use systemd-server runtime auto-detection.
- Document that transcript message/path exposure becomes near-real-time and is intentionally controlled by the existing scrub/query policy.

## Non-Duplication Rules

- The watcher must call `SyslogService::add_ai_file(file, false)` or `SyslogService::index_ai_roots(path, false, None)`. It must not parse JSONL records itself.
- The watcher must use scanner-owned helpers for default roots and supported file extensions.
- The watcher must not write `transcript_sources`, `transcript_import_records`, or `transcript_parse_errors` directly.
- `syslog setup ai-watch-service install` disables `syslog-ai-index.timer`. The timer remains as an optional fallback command only.
- Any scanner optimization must live inside `src/scanner.rs` / `src/scanner/checkpoint.rs`, not in `src/ai_watch.rs`.

## File Map

| File | Action | Responsibility |
| --- | --- | --- |
| `Cargo.toml`, `Cargo.lock` | Modify | Add `notify = "8.2.0"` and bump version |
| `src/scanner.rs` | Modify | Export root/file helpers; add append-offset fast path for unchanged prefix/appends |
| `src/scanner/checkpoint.rs` | Modify | Expose checkpoint metadata needed for append-offset scans |
| `src/scanner_tests.rs`, `src/scanner/checkpoint_tests.rs` | Modify | Cover append-only indexing, rewrite fallback, retry-safe checkpoint behavior |
| `src/ai_watch.rs` | Create | Bounded/coalesced watcher adapter, stable-file retry, non-fatal per-file errors, SIGTERM handling |
| `src/lib.rs` | Modify | Export `ai_watch` |
| `src/cli.rs`, `src/cli_tests.rs` | Modify | Add `syslog ai watch`; parse nonzero timing args; route to watcher |
| `src/main.rs`, `src/main_tests.rs` | Modify | Add usage and setup parser coverage |
| `src/setup.rs`, `src/setup_tests.rs` | Modify | Add `setup ai-watch-service`; resolve binary/DB; hardened unit/env; timer-disable checks |
| `README.md`, `docs/CLI.md` | Modify | Document real-time watcher, fallback timer, exposure policy, troubleshooting |
| `CHANGELOG.md`, `.claude-plugin/plugin.json` | Modify | Version and release notes |

---

## Task 1: Pin Notify and Add CLI Shape

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `src/cli.rs`
- Modify: `src/cli_tests.rs`
- Modify: `src/main.rs`

- [x] **Step 1: Add the MSRV-compatible dependency**

Run:

```bash
cargo add notify@8.2.0
```

Expected: `Cargo.toml` contains `notify = "8.2.0"`. Do not use `EventKindMask`; it is not available in this stable API.

- [x] **Step 2: Add failing parser tests**

Add to `src/cli_tests.rs`:

```rust
#[test]
fn parse_ai_watch_defaults() {
    let command = CliCommand::parse(strings(&["ai", "watch"])).unwrap();
    assert_eq!(
        command,
        CliCommand::Ai(AiCommand::Watch(AiWatchArgs {
            path: None,
            debounce_ms: 750,
            settle_ms: 500,
            max_retries: 5,
            no_initial_scan: false,
            json: false,
        }))
    );
}

#[test]
fn parse_ai_watch_all_options() {
    let command = CliCommand::parse(strings(&[
        "ai",
        "watch",
        "--path",
        "/tmp/transcripts",
        "--debounce-ms",
        "100",
        "--settle-ms=250",
        "--max-retries=7",
        "--no-initial-scan",
        "--json",
    ]))
    .unwrap();
    assert_eq!(
        command,
        CliCommand::Ai(AiCommand::Watch(AiWatchArgs {
            path: Some("/tmp/transcripts".into()),
            debounce_ms: 100,
            settle_ms: 250,
            max_retries: 7,
            no_initial_scan: true,
            json: true,
        }))
    );
}

#[test]
fn parse_ai_watch_rejects_zero_timing_values() {
    let err = CliCommand::parse(strings(&["ai", "watch", "--debounce-ms", "0"])).unwrap_err();
    assert!(err.to_string().contains("positive integer"));
}
```

- [x] **Step 3: Add CLI types and parser**

In `src/cli.rs`, add `Watch(AiWatchArgs)` to `AiCommand` and add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiWatchArgs {
    pub path: Option<String>,
    pub debounce_ms: u64,
    pub settle_ms: u64,
    pub max_retries: u8,
    pub no_initial_scan: bool,
    pub json: bool,
}

impl Default for AiWatchArgs {
    fn default() -> Self {
        Self {
            path: None,
            debounce_ms: 750,
            settle_ms: 500,
            max_retries: 5,
            no_initial_scan: false,
            json: false,
        }
    }
}
```

Add `"watch" => parse_ai_watch(rest),` to `parse_ai`, plus:

```rust
fn parse_positive_u64_flag(flag: &str, value: String) -> Result<u64> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| anyhow!("{flag} expects a positive integer"))?;
    if parsed == 0 {
        bail!("{flag} expects a positive integer");
    }
    Ok(parsed)
}

fn parse_ai_watch(args: &[String]) -> Result<CliCommand> {
    let mut parsed = AiWatchArgs::default();
    let mut flags = FlagCursor::new(args);
    while let Some(arg) = flags.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--path" => parsed.path = Some(flags.value("--path")?),
            "--debounce-ms" => {
                parsed.debounce_ms =
                    parse_positive_u64_flag("--debounce-ms", flags.value("--debounce-ms")?)?;
            }
            "--settle-ms" => {
                parsed.settle_ms =
                    parse_positive_u64_flag("--settle-ms", flags.value("--settle-ms")?)?;
            }
            "--max-retries" => {
                parsed.max_retries = parse_u32_flag("--max-retries", flags.value("--max-retries")?)?
                    .try_into()
                    .map_err(|_| anyhow!("--max-retries is too large"))?;
            }
            "--no-initial-scan" => parsed.no_initial_scan = true,
            _ if arg.starts_with("--path=") => {
                parsed.path = Some(value_after_equals(arg, "--path")?)
            }
            _ if arg.starts_with("--debounce-ms=") => {
                parsed.debounce_ms =
                    parse_positive_u64_flag("--debounce-ms", value_after_equals(arg, "--debounce-ms")?)?;
            }
            _ if arg.starts_with("--settle-ms=") => {
                parsed.settle_ms =
                    parse_positive_u64_flag("--settle-ms", value_after_equals(arg, "--settle-ms")?)?;
            }
            _ if arg.starts_with("--max-retries=") => {
                parsed.max_retries = parse_u32_flag(
                    "--max-retries",
                    value_after_equals(arg, "--max-retries")?,
                )?
                .try_into()
                .map_err(|_| anyhow!("--max-retries is too large"))?;
            }
            _ => bail!("unknown ai watch option: {arg}"),
        }
    }
    Ok(CliCommand::Ai(AiCommand::Watch(parsed)))
}
```

- [x] **Step 4: Add temporary runner arm and usage**

Add this temporary arm in `cli::run`:

```rust
AiCommand::Watch(_) => bail!("ai watch is parsed but not implemented yet"),
```

Add to `src/main.rs` usage:

```text
  syslog ai watch [--path PATH] [--debounce-ms N] [--settle-ms N] [--max-retries N] [--no-initial-scan] [--json]
```

- [x] **Step 5: Verify**

Run:

```bash
cargo test cli::tests::parse_ai_watch_defaults cli::tests::parse_ai_watch_all_options cli::tests::parse_ai_watch_rejects_zero_timing_values
```

Expected: all pass.

---

## Task 2: Make Scanner Policy Reusable and Add Append-Offset Indexing

**Files:**
- Modify: `src/scanner.rs`
- Modify: `src/scanner/checkpoint.rs`
- Modify: `src/scanner_tests.rs`
- Modify: `src/scanner/checkpoint_tests.rs`

- [x] **Step 1: Add failing scanner helper tests**

Add to `src/scanner_tests.rs`:

```rust
#[test]
fn scanner_exposes_default_roots_and_supported_file_policy() {
    let roots = default_transcript_roots();
    assert!(roots.iter().any(|path| path.ends_with(".claude/projects")));
    assert!(roots.iter().any(|path| path.ends_with(".codex/sessions")));
    assert!(is_supported_transcript_file(std::path::Path::new("session.jsonl")));
    assert!(!is_supported_transcript_file(std::path::Path::new("session.json")));
}
```

- [x] **Step 2: Export scanner-owned helpers**

In `src/scanner.rs`, make the existing policy public:

```rust
pub fn is_supported_transcript_file(path: &Path) -> bool {
    supported_discovered_file(path)
}

pub fn default_transcript_roots() -> Vec<PathBuf> {
    default_roots()
}
```

Keep `supported_discovered_file` and `default_roots` as the internal implementation so existing scanner behavior remains unchanged.

- [x] **Step 3: Add append-only indexing tests**

Add to `src/scanner_tests.rs`:

```rust
#[test]
fn append_only_file_indexes_only_new_records_after_checkpoint() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("session.jsonl");
    std::fs::write(
        &file,
        "{\"uuid\":\"one\",\"type\":\"user\",\"timestamp\":\"2026-05-14T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"first\"}}\n",
    )
    .unwrap();

    let first = index_file(&pool, &file, "explicit_file").unwrap();
    assert_eq!(first.ingested, 1);

    let mut open = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
    use std::io::Write;
    writeln!(
        open,
        "{{\"uuid\":\"two\",\"type\":\"user\",\"timestamp\":\"2026-05-14T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":\"second\"}}}}"
    )
    .unwrap();

    let second = index_file(&pool, &file, "explicit_file").unwrap();
    assert_eq!(second.ingested, 1);
    assert_eq!(second.skipped_dupes, 0);
}

#[test]
fn rewritten_file_falls_back_to_duplicate_safe_full_scan() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("session.jsonl");
    std::fs::write(
        &file,
        "{\"uuid\":\"one\",\"type\":\"user\",\"timestamp\":\"2026-05-14T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"first\"}}\n",
    )
    .unwrap();
    assert_eq!(index_file(&pool, &file, "explicit_file").unwrap().ingested, 1);

    std::fs::write(
        &file,
        "{\"uuid\":\"one\",\"type\":\"user\",\"timestamp\":\"2026-05-14T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"first changed\"}}\n",
    )
    .unwrap();

    let second = index_file(&pool, &file, "explicit_file").unwrap();
    assert_eq!(second.ingested, 0);
    assert!(second.skipped_dupes >= 1);
}
```

- [x] **Step 4: Implement scanner append fast path**

Use existing checkpoint metadata. Add a `source_metadata(source_id)` method to `CheckpointStore` returning file size, mtime, hash, and last offset. In `index_file_with_options`, if not `force`, metadata exists, current size is greater than old size, current mtime is newer/equal, and the existing file prefix is still valid, seek to `last_offset` and parse only appended lines. If the file shrank, metadata is missing, or the prefix validation cannot be proven cheaply, fall back to the existing full scan. Do not change record key semantics.

Use `std::io::{Seek, SeekFrom}` and start line numbering from the stored last offset context only if the checkpoint store can return the previous imported count safely. If line numbering cannot be preserved, keep record keys content/session based for parsed records; do not introduce offset-based keys for the append path.

After successful append parse with no parse errors, update source metadata and `last_offset` in the same transaction as imports/log rows. On parse errors, do not update completion metadata; watcher will retry.

- [x] **Step 5: Add transient SQLite retry for scanner chunks**

Wrap `flush_chunk` transaction work in the same transient-lock retry policy used by syslog batch ingest. If the existing retry helper is private, extract a small shared helper in `src/db/ingest.rs` or retry inside scanner for `SQLITE_BUSY`/`SQLITE_LOCKED`. Tests should prove a retryable error does not advance checkpoint metadata before successful insert.

- [x] **Step 6: Verify**

Run:

```bash
cargo test scanner::tests::scanner_exposes_default_roots_and_supported_file_policy scanner::tests::append_only_file_indexes_only_new_records_after_checkpoint scanner::tests::rewritten_file_falls_back_to_duplicate_safe_full_scan scanner::checkpoint
```

Expected: all pass.

---

## Task 3: Implement Bounded, Coalesced Watcher Adapter

**Files:**
- Create: `src/ai_watch.rs`
- Modify: `src/lib.rs`
- Modify: `src/cli.rs`
- Test: `src/ai_watch.rs`

- [x] **Step 1: Add watcher state tests first**

Create `src/ai_watch.rs` with state tests:

```rust
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct PendingFile {
    first_seen: Instant,
    last_seen: Instant,
    retries: u8,
    last_len: Option<u64>,
    last_mtime: Option<std::time::SystemTime>,
}

#[derive(Debug, Default)]
struct PendingFiles {
    files: BTreeMap<PathBuf, PendingFile>,
    rescan_needed: bool,
    coalesced_events: u64,
}

impl PendingFiles {
    fn push(&mut self, path: PathBuf, now: Instant) {
        self.files
            .entry(path)
            .and_modify(|entry| {
                entry.last_seen = now;
                self.coalesced_events += 1;
            })
            .or_insert(PendingFile {
                first_seen: now,
                last_seen: now,
                retries: 0,
                last_len: None,
                last_mtime: None,
            });
    }

    fn requeue(&mut self, path: PathBuf, now: Instant, max_retries: u8) -> bool {
        let entry = self.files.entry(path).or_insert(PendingFile {
            first_seen: now,
            last_seen: now,
            retries: 0,
            last_len: None,
            last_mtime: None,
        });
        if entry.retries >= max_retries {
            return false;
        }
        entry.retries += 1;
        entry.last_seen = now;
        true
    }

    fn ready(&self, now: Instant, debounce: Duration) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter_map(|(path, entry)| {
                (now.duration_since(entry.last_seen) >= debounce).then(|| path.clone())
            })
            .collect()
    }

    fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_files_deduplicate_and_requeue_with_cap() {
        let start = Instant::now();
        let path = PathBuf::from("/tmp/session.jsonl");
        let mut pending = PendingFiles::default();
        pending.push(path.clone(), start);
        pending.push(path.clone(), start + Duration::from_millis(25));
        assert_eq!(pending.files.len(), 1);
        assert!(pending.ready(start + Duration::from_millis(100), Duration::from_millis(200)).is_empty());
        assert_eq!(pending.ready(start + Duration::from_millis(300), Duration::from_millis(200)), vec![path.clone()]);
        assert!(pending.requeue(path.clone(), start + Duration::from_millis(301), 1));
        assert!(!pending.requeue(path, start + Duration::from_millis(302), 1));
    }
}
```

- [x] **Step 2: Export module**

In `src/lib.rs`, add:

```rust
pub mod ai_watch;
```

- [x] **Step 3: Implement watcher API**

Use `notify 8.2.0`:

```rust
use anyhow::{Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::{app::SyslogService, scanner};

#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub path: Option<PathBuf>,
    pub debounce: Duration,
    pub settle: Duration,
    pub max_retries: u8,
    pub initial_scan: bool,
    pub json: bool,
}
```

Required behavior:
- Build roots from `options.path` or `scanner::default_transcript_roots()`.
- Register `RecommendedWatcher::new(callback, Config::default().with_follow_symlinks(false))`.
- Use `mpsc::channel(1024)` and `try_send` in the callback. If the channel is full, set an `Arc<AtomicBool>` rescan flag.
- Register watches before any initial scan.
- If initial scan is enabled, run it after watch registration.
- Check `event.need_rescan()` before create/modify/name handling.
- Treat create, modify, and rename-to/name events for scanner-supported `.jsonl` files as candidates.
- Ignore remove events except debug logging.
- Evaluate file stability on ticks without serial per-file sleeps. Store `(len, modified)` snapshots in `PendingFiles`; a file is stable after the same metadata appears across two ticks separated by `settle`.
- Process stable files best-effort. Per-file metadata errors, scanner errors, parse-error results, and storage-blocked results log warnings and either requeue with backoff or drop after `max_retries`; they must not return from `run`.
- On parse errors from `IndexResult`, requeue because watcher-triggered parse errors may be partial trailing JSONL.
- Bound concurrent scans to 2 with a semaphore or keep serial scans after eliminating serial settle sleeps. Do not allow unbounded spawned tasks.
- Handle Ctrl-C and SIGTERM.

- [x] **Step 4: Wire CLI**

Replace the temporary arm:

```rust
AiCommand::Watch(args) => {
    let options = syslog_mcp::ai_watch::WatchOptions {
        path: args.path.map(std::path::PathBuf::from),
        debounce: std::time::Duration::from_millis(args.debounce_ms),
        settle: std::time::Duration::from_millis(args.settle_ms),
        max_retries: args.max_retries,
        initial_scan: !args.no_initial_scan,
        json: args.json,
    };
    syslog_mcp::ai_watch::run(service, options).await?;
}
```

- [x] **Step 5: Verify**

Run:

```bash
cargo test ai_watch::tests cli::tests::parse_ai_watch_defaults cli::tests::parse_ai_watch_all_options cli::tests::parse_ai_watch_rejects_zero_timing_values
cargo check
```

Expected: all pass.

---

## Task 4: Add Hardened Host-Local Watch Service Setup

**Files:**
- Modify: `src/setup.rs`
- Modify: `src/setup_tests.rs`
- Modify: `src/main.rs`
- Modify: `src/main_tests.rs`

- [x] **Step 1: Add setup tests**

Add tests that assert:
- `ai_watch_env_file` includes a canonical `SYSLOG_MCP_DB_PATH`.
- `ai_watch_service_unit` uses absolute `ExecStart`, `EnvironmentFile`, `UMask=0077`, `Restart=on-failure`, restart limits, `NoNewPrivileges=true`, `PrivateTmp=true`, `ProtectSystem=strict`, `BindReadOnlyPaths` for transcript roots, and `ReadWritePaths` for DB/state directories.
- Install phases disable `syslog-ai-index.timer` before enabling `syslog-ai-watch.service`.
- Timer absence is treated as OK, but active/enabled timer after install/check is an error.
- `systemctl_user_phase` still uses the DBUS/XDG fallback already present in this branch.
- The setup parser accepts `syslog setup ai-watch-service install|remove|check [--json]`.

- [x] **Step 2: Add setup action and runner**

In `src/setup.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiWatchServiceAction {
    Install,
    Remove,
    Check,
}
```

Add `run_ai_watch_service_setup(action)` mirroring `run_ai_index_timer_setup`, but with:
- `~/.config/syslog-mcp/ai-watch.env` for environment.
- `~/.config/systemd/user/syslog-ai-watch.service`.
- No shell lookup of `syslog` at runtime.
- Resolved absolute binary from `std::env::current_exe()` when invoked through the intended wrapper, or from `command -v syslog` at install time followed by canonicalization and permission checks.
- Resolved DB path from explicit `SYSLOG_MCP_DB_PATH` if set, else the known live plugin DB if it exists. If multiple plausible DBs exist and no explicit value is set, return a warning/error requiring `SYSLOG_MCP_DB_PATH`.

- [x] **Step 3: Generate hardened unit**

The unit should look structurally like:

```text
[Unit]
Description=syslog-mcp real-time local AI transcript watch
Documentation=https://github.com/jmagar/syslog-mcp
After=default.target
StartLimitIntervalSec=300
StartLimitBurst=5

[Service]
Type=simple
EnvironmentFile=%h/.config/syslog-mcp/ai-watch.env
WorkingDirectory=/
ExecStart=/absolute/path/to/syslog ai watch --no-initial-scan --json
Restart=on-failure
RestartSec=5
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
BindReadOnlyPaths=-%h/.claude/projects -%h/.codex/sessions
BindPaths=/absolute/db/dir %h/.local/state/syslog-mcp
ReadWritePaths=/absolute/db/dir %h/.local/state/syslog-mcp

[Install]
WantedBy=default.target
```

If `ProtectSystem=strict` or `BindReadOnlyPaths` is not portable in this user service environment, include it in the plan tests as generated text and allow the live check to report systemd incompatibility clearly.

- [x] **Step 4: Install flow**

Install must:
1. Write the env file with `SYSLOG_MCP_DB_PATH`, `SYSLOG_DOCKER_INGEST_ENABLED=false`, `RUST_LOG=warn`.
2. Write the unit.
3. Run one explicit `syslog ai index --json` with the resolved DB path before enabling the no-initial-scan service.
4. `systemctl --user daemon-reload`.
5. `systemctl --user disable --now syslog-ai-index.timer`, treating missing timer as OK.
6. Verify timer inactive/disabled or absent.
7. `systemctl --user enable --now syslog-ai-watch.service`.

- [x] **Step 5: Wire `src/main.rs` setup parser**

Add `SetupCommandKind::AiWatchService(AiWatchServiceAction)` and parse:

```text
syslog setup ai-watch-service install|remove|check [--json]
```

Update usage.

- [x] **Step 6: Verify**

Run:

```bash
cargo test setup_tests main_tests::parse_setup_ai_watch_service
```

Expected: setup parser and generated files pass.

---

## Task 5: Document Real-Time Ingestion and Exposure Policy

**Files:**
- Modify: `README.md`
- Modify: `docs/CLI.md`
- Modify: `CHANGELOG.md`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `.claude-plugin/plugin.json`

- [x] **Step 1: Update docs**

Document:
- `syslog ai watch` watches scanner-owned transcript roots and reuses `syslog ai add` semantics.
- `syslog setup ai-watch-service install` is the primary local helper.
- `syslog setup ai-index-timer` is only a polling fallback.
- Installing the watcher disables the polling timer.
- The watcher is host-local and not inside the Docker Compose container.
- Transcript messages and transcript paths become searchable in near real time through existing query surfaces; scrubbing is best-effort, not a compliance boundary.
- Troubleshooting: user systemd DBUS/XDG issues, inotify watch limits, service logs, and DB path mismatch.

- [x] **Step 2: Bump version**

Run:

```bash
bash scripts/bump-version.sh patch
```

Expected: `0.21.6` becomes `0.21.7` in all version-bearing files.

- [x] **Step 3: Add changelog entry**

Add under `0.21.7`:

```markdown
- **Real-time AI transcript ingestion**: Added `syslog ai watch` and
  `syslog setup ai-watch-service` for host-local filesystem watching of Claude
  and Codex JSONL transcripts. The watcher reuses scanner checkpoints, dedupe,
  storage checks, parse-error tracking, and append-offset indexing while
  disabling the older polling timer during service install.
```

- [x] **Step 4: Verify docs consistency**

Run:

```bash
rg -n "ai-index-timer|ai-watch-service|ai watch|30min|30 min|OnUnitActiveSec" README.md docs/CLI.md CHANGELOG.md src/setup.rs
bash scripts/check-version-sync.sh
```

Expected: docs identify watcher as primary and timer as fallback.

---

## Task 6: Full Verification and Live Operational Proof

**Files:**
- Modify only if verification exposes a defect.

- [x] **Step 1: Static/full local verification**

Run:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
bash scripts/check-version-sync.sh
```

- [x] **Step 2: Compose-specific runtime verification**

Run:

```bash
syslog compose doctor --json
syslog compose status --json
bash scripts/check-runtime-current.sh --mode docker
syslog compose logs --tail 100
curl -sf http://localhost:3100/health
bash scripts/smoke-test.sh
```

Do not use runtime auto-detection that can prefer a removed server-side systemd unit.

- [x] **Step 3: Install and inspect watcher service**

Run:

```bash
syslog setup ai-watch-service install --json
syslog setup ai-watch-service check --json
systemctl --user is-active syslog-ai-watch.service
systemctl --user is-enabled syslog-ai-watch.service
systemctl --user is-active syslog-ai-index.timer || true
systemctl --user is-enabled syslog-ai-index.timer || true
systemctl --user show syslog-ai-watch.service -p NRestarts -p ExecMainStatus --no-pager
journalctl --user -u syslog-ai-watch.service -n 100 --no-pager
```

Expected: watcher active/enabled, timer inactive/disabled or absent, no restart loop, no DB open errors.

- [x] **Step 4: Prove real-time ingestion with a disposable transcript**

Run:

```bash
SMOKE_ID="syslog-realtime-watch-$(date +%s)"
SMOKE_DIR="$HOME/.claude/projects/-tmp-syslog-watch-smoke"
SMOKE_FILE="$SMOKE_DIR/$SMOKE_ID.jsonl"
mkdir -p "$SMOKE_DIR"
printf '{"uuid":"%s","type":"user","timestamp":"%s","message":{"role":"user","content":"%s"}}\n' "$SMOKE_ID" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$SMOKE_ID" > "$SMOKE_FILE"
for i in $(seq 1 20); do
  syslog ai search "\"$SMOKE_ID\"" --limit 5 --json | tee /tmp/syslog-watch-smoke.json
  rg -q "$SMOKE_ID" /tmp/syslog-watch-smoke.json && break
  sleep 1
done
rg -q "$SMOKE_ID" /tmp/syslog-watch-smoke.json
rg -q "$(basename "$SMOKE_FILE")" /tmp/syslog-watch-smoke.json
```

Expected: the row appears without running `syslog ai index` manually. Assert by unique message and transcript filename, not by an assumed decoded project path.

- [x] **Step 5: Prove duplicate prevention and cleanup**

Run:

```bash
syslog ai add --file "$SMOKE_FILE" --json
syslog ai add --file "$SMOKE_FILE" --json
rm -f "$SMOKE_FILE"
rmdir "$SMOKE_DIR" 2>/dev/null || true
syslog ai doctor --json
```

Expected: duplicate run ingests zero new rows or reports duplicates; doctor shows no parse errors introduced by the smoke.

- [x] **Step 6: Commit and push**

Run:

```bash
git status --short
git add .
git commit -m "feat: add real-time AI transcript ingestion"
git push -u origin HEAD
```

---

## Self-Review

- Spec coverage: real-time ingestion, no duplicated scanner logic, host-local deployment, Docker Compose-only server runtime, timer replacement, DB path proof, systemd hardening, live smoke, and review feedback are all represented.
- Placeholder scan: no unfinished placeholder text remains.
- Type consistency: `AiWatchArgs`, `WatchOptions`, `AiCommand::Watch`, `syslog ai watch`, and `syslog setup ai-watch-service` are consistent across CLI, setup, docs, and verification.
