# mnemo Feature Port Fully Operational Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the already-merged mnemo AI-session feature port so `syslog ai` indexing and AI analytics are safe, bounded, documented, and production-operational.

**Architecture:** Keep the existing public query surfaces and the existing `logs` table source of truth. Harden the local transcript scanner around explicit path policy, streaming JSONL parsing, chunked transactions, storage-budget enforcement, and checkpoint semantics. Tighten analytics response limits, parser fidelity, docs, and seeded live verification without introducing a separate sessions table.

**Tech Stack:** Rust, SQLite + FTS5 via `rusqlite`, `r2d2`, `serde_json`, `sha2`, `axum`/RMCP HTTP, direct CLI over `SyslogService`, shell smoke tests.

---

## Current Status

Implemented and working now:

- `syslog ai search`, `blocks`, `context`, `tools`, `projects`, `index`, and `add` are parsed and routed through `src/cli.rs`.
- MCP actions `search_sessions`, `usage_blocks`, `project_context`, `list_ai_tools`, and `list_ai_projects` dispatch through `src/mcp/tools.rs`.
- AI DB models and analytics exist in `src/db/models.rs`, `src/db/queries.rs`, and `src/db/analytics.rs`.
- Transcript source and import-record tables exist through migrations 5 and 6 in `src/db/pool.rs`.
- Happy-path indexing works for explicit files, default roots, idempotent duplicate skips, secret scrubbing, and basic Claude/Codex JSONL shapes.
- `cargo test` and `cargo clippy --all-targets --all-features -- -D warnings` passed before this plan was written.

Remaining operational gaps found in the code review:

- `src/scanner.rs` reads each transcript file fully into memory with `fs::read_to_string`; the plan requires incremental JSONL parsing and bounded chunking.
- `src/scanner.rs` accepts any explicit `--path` that exists; broad paths like `/`, `$HOME`, and the repository root are not rejected before recursive scanning.
- `src/scanner.rs` does not enforce storage-budget/write-block semantics before committing transcript chunks.
- `src/scanner/checkpoint.rs` sets `last_indexed_at` when creating a source and `mark_error` also updates `last_indexed_at`; checkpoint timestamps can move without successful imports.
- Parse errors are counted, but checkpoint/update semantics around parse failure are not strict enough to prove "do not advance on parsing, storage checks, insert, or FTS failure".
- `IndexResult` lacks the fields required for operational JSON summaries: skipped symlink counts, unsafe path counts, unsupported file counts, storage-blocked chunks, and checkpoint state changes.
- Codex parsing can confuse response item ids with session ids unless session metadata is carried as file-level scanner state.
- Claude parsing misses common content-array object shapes such as `{ "type": "text", "text": "..." }`.
- Scanner metadata is not length-capped consistently for project, session id, transcript path, and message record size.
- AI analytics have hard limits but incomplete truncation metadata, and some unbounded inventories/context aggregates can scan all retained rows without an explicit bounded default or documented all-retained behavior.
- `project_context` returns full `LogEntry` objects; representative output is bounded by row count but not by message/snippet length.
- MCP schema text still describes `query` as only for `search` and `correlate`; it must mention `search_sessions`.
- Docs say transcript rows have no automatic redaction, while scanner code runs `scrub_ai_message`; docs must state the exact redaction and raw visibility policy.
- Smoke/live tests validate response shape but do not seed AI transcript data and prove non-empty results across CLI and HTTP MCP.

## File Map

| File | Action | Responsibility |
| --- | --- | --- |
| `src/scanner.rs` | Modify | Path policy, streaming reader, chunked indexing, storage preflight, expanded `IndexResult` |
| `src/scanner/checkpoint.rs` | Modify | Source lookup/create without advancing timestamps; transactional checkpoint updates |
| `src/scanner/claude.rs` | Modify | Parse Claude content string, content arrays, nested message content, session/project metadata |
| `src/scanner/codex.rs` | Modify | Parse Codex transcript records using scanner file context for session/project identity |
| `src/scanner_tests.rs` | Modify | End-to-end scanner tests for unsafe paths, chunking, storage block, checkpoint failure, FTS duplicates |
| `src/scanner/checkpoint_tests.rs` | Modify | Transactional checkpoint tests and timestamp semantics |
| `src/scanner/claude_tests.rs` | Modify | Claude realistic content-shape parser tests |
| `src/scanner/codex_tests.rs` | Modify | Codex session metadata and response item parser tests |
| `src/app/models.rs` | Modify | Add analytics truncation metadata and representative snippet shape |
| `src/app/service.rs` | Modify | Pass `StorageConfig` into scanner and normalize new model fields |
| `src/db/models.rs` | Modify | Add DB result metadata for inventory/context truncation |
| `src/db/queries.rs` | Modify | Add bounded defaults/truncation for `search_ai_sessions`, `list_ai_tools`, `list_ai_projects` |
| `src/db/analytics.rs` | Modify | Add bounded defaults/truncation for `usage_blocks` and bounded representative snippets for `project_context` |
| `src/db/queries_tests.rs` | Modify | Add cap/truncation, FTS quirks, and filtered event-count tests |
| `src/db/analytics_tests.rs` | Modify | Add usage-block default-window and project-context representative-limit tests |
| `src/mcp/schemas.rs` | Modify | Update schema descriptions for AI actions and result limits |
| `src/mcp/tools.rs` | Modify | Update help text for exact semantics and redaction/raw visibility policy |
| `src/cli.rs` | Modify | Print richer index result fields and make `truncate` UTF-8 safe |
| `src/cli_tests.rs` | Modify | Add parser/output tests for unsafe indexing errors and Unicode truncation |
| `src/otlp.rs` | Modify | Keep AI attribute trust contract explicit and length caps consistent |
| `src/otlp_tests.rs` | Modify | Add oversized project/session path and producer-supplied trust tests |
| `docs/CLI.md` | Modify | Document indexing path policy, rerun behavior, redaction, storage block behavior |
| `docs/mcp/TOOLS.md` | Modify | Document raw transcript visibility and scrubbed-message policy accurately |
| `docs/mcp/SCHEMA.md` | Modify | Regenerate/update schema action docs for AI action arguments |
| `docs/mcp/TESTS.md` | Modify | Add seeded AI smoke/live coverage requirements |
| `README.md` | Modify | Add operational AI-session indexing section |
| `docs/expansion.md` | Modify | Replace stale "missing/future" transcript notes with current status and remaining ops constraints |
| `scripts/smoke-test.sh` | Modify | Seed AI transcript data before AI action shape checks |
| `tests/test_live.sh` | Modify | Seed AI transcript data and assert non-empty AI action results |
| `tests/mcporter/test-tools.sh` | Modify | Seed or require fixture data for AI action result checks |

---

## Task 1: Lock Down Scanner Path Policy and Result Shape

**Files:**
- Modify: `src/scanner.rs`
- Modify: `src/scanner_tests.rs`
- Modify: `src/cli.rs`
- Test: `src/scanner_tests.rs`
- Test: `src/cli_tests.rs`

- [ ] **Step 1: Write failing tests for unsafe paths and result counters**

Add these tests to `src/scanner_tests.rs`:

```rust
#[test]
fn index_roots_rejects_broad_home_root_and_repo_paths() {
    let (pool, _dir) = test_pool();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let repo = std::env::current_dir().unwrap();

    for path in [std::path::Path::new("/"), home.as_path(), repo.as_path()] {
        let result = index_roots(&pool, Some(path)).unwrap();
        assert_eq!(result.ingested, 0, "unsafe path ingested rows: {}", path.display());
        assert_eq!(result.skipped_unsafe_paths, 1, "unsafe path not counted: {}", path.display());
        assert_eq!(result.discovered_files, 0, "unsafe path was scanned: {}", path.display());
    }
}

#[test]
fn index_roots_counts_unsupported_files_without_parsing_them() {
    let (pool, dir) = test_pool();
    std::fs::write(dir.path().join("notes.txt"), "not a transcript").unwrap();
    std::fs::write(
        dir.path().join("session.jsonl"),
        "{\"sessionId\":\"safe\",\"content\":\"indexed\"}\n",
    )
    .unwrap();

    let result = index_roots(&pool, Some(dir.path())).unwrap();

    assert_eq!(result.ingested, 1);
    assert_eq!(result.unsupported_files, 1);
    assert_eq!(result.skipped_unsafe_paths, 0);
}
```

Add this test to `src/cli_tests.rs` near the existing parser tests:

```rust
#[test]
fn truncate_is_utf8_safe_for_non_ascii_project_names() {
    let value = super::truncate("项目路径-alpha", 6);
    assert!(value.ends_with('…'));
    assert!(value.is_char_boundary(value.len()));
}
```

- [ ] **Step 2: Run the new tests and confirm failure**

Run:

```bash
cargo test scanner::tests::index_roots_rejects_broad_home_root_and_repo_paths scanner::tests::index_roots_counts_unsupported_files_without_parsing_them cli::tests::truncate_is_utf8_safe_for_non_ascii_project_names
```

Expected: compile failures for missing `IndexResult` fields and test failure or panic for current byte-slicing `truncate`.

- [ ] **Step 3: Expand `IndexResult` and add path policy helpers**

In `src/scanner.rs`, replace `IndexResult` with:

```rust
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct IndexResult {
    pub discovered_files: usize,
    pub ingested: usize,
    pub skipped_dupes: usize,
    pub parse_errors: usize,
    pub skipped_files: usize,
    pub unsupported_files: usize,
    pub skipped_symlinks: usize,
    pub skipped_unsafe_paths: usize,
    pub storage_blocked_chunks: usize,
    pub checkpoint_updates: usize,
    pub file_errors: Vec<IndexFileError>,
}
```

Add this helper in `src/scanner.rs`:

```rust
fn classify_path_error(error: &anyhow::Error, result: &mut IndexResult) {
    let message = error.to_string();
    if message.contains("symlinks are not allowed") {
        result.skipped_symlinks += 1;
    }
    if message.contains("unsafe transcript scan path") {
        result.skipped_unsafe_paths += 1;
    }
}

fn is_known_transcript_root(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let allowed = [
        home.join(".claude/projects"),
        home.join(".codex/sessions"),
    ];
    allowed.iter().any(|root| path == root || path.starts_with(root))
}

fn reject_broad_scan_path(path: &Path) -> Result<()> {
    let canonical = path.canonicalize()?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    if canonical == Path::new("/")
        || home.as_ref().is_some_and(|value| &canonical == value)
        || cwd.as_ref().is_some_and(|value| &canonical == value)
    {
        bail!("unsafe transcript scan path: {}", canonical.display());
    }
    if !is_known_transcript_root(&canonical) {
        let supported_file = canonical.is_file() && supported_discovered_file(&canonical);
        if !supported_file {
            bail!(
                "unsafe transcript scan path: {}; pass a known transcript root or one .jsonl file",
                canonical.display()
            );
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Apply the path policy before scanning**

In `index_roots`, replace the `Some(path)` arm with:

```rust
Some(path) => {
    let mut result = IndexResult::default();
    if let Err(error) = validate_path(path).and_then(|_| reject_broad_scan_path(path)) {
        classify_path_error(&error, &mut result);
        result.skipped_files += 1;
        result.file_errors.push(IndexFileError {
            path: path.display().to_string(),
            error: error.to_string(),
        });
        return Ok(result);
    }
    vec![path.to_path_buf()]
}
```

Update every `file_errors.push(...)` path in `collect_supported_files` and `index_roots` to call `classify_path_error(&error, result)` before pushing the error.

Update `collect_supported_files` so unsupported regular files are counted:

```rust
if path.is_file() {
    if supported_discovered_file(path) {
        files.push(path.to_path_buf());
    } else {
        result.unsupported_files += 1;
    }
    return;
}
```

Inside the directory loop, replace the final `else if` with:

```rust
} else if supported_discovered_file(&entry) {
    files.push(entry);
} else if entry.is_file() {
    result.unsupported_files += 1;
}
```

- [ ] **Step 5: Make CLI truncation UTF-8 safe and print richer index results**

Replace `truncate` in `src/cli.rs` with:

```rust
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let prefix: String = s.chars().take(max - 1).collect();
    format!("{prefix}…")
}
```

Replace the human output in `print_index_response` with:

```rust
println!(
    "files={} ingested={} duplicates={} parse_errors={} skipped={} unsupported={} symlinks={} unsafe_paths={} storage_blocked_chunks={} checkpoint_updates={} file_errors={}",
    response.discovered_files,
    response.ingested,
    response.skipped_dupes,
    response.parse_errors,
    response.skipped_files,
    response.unsupported_files,
    response.skipped_symlinks,
    response.skipped_unsafe_paths,
    response.storage_blocked_chunks,
    response.checkpoint_updates,
    response.file_errors.len()
);
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test scanner::tests::index_roots_rejects_broad_home_root_and_repo_paths scanner::tests::index_roots_counts_unsupported_files_without_parsing_them cli::tests::truncate_is_utf8_safe_for_non_ascii_project_names
```

Expected: all three tests pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/scanner.rs src/scanner_tests.rs src/cli.rs src/cli_tests.rs
git commit -m "fix: harden transcript scanner path policy"
```

---

## Task 2: Make Scanner Streaming, Chunked, and Metadata-Capped

**Files:**
- Modify: `src/scanner.rs`
- Modify: `src/scanner_tests.rs`

- [ ] **Step 1: Write failing tests for record size, chunking, and metadata caps**

Add to `src/scanner_tests.rs`:

```rust
#[test]
fn index_file_rejects_oversized_jsonl_record_without_inserting_any_rows() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("oversized.jsonl");
    let large = "x".repeat(super::MAX_RECORD_SIZE_BYTES + 1);
    std::fs::write(
        &file,
        format!("{{\"sessionId\":\"s1\",\"content\":\"{large}\"}}\n"),
    )
    .unwrap();

    let result = index_file(&pool, &file, "explicit_file").unwrap();
    assert_eq!(result.ingested, 0);
    assert_eq!(result.parse_errors, 1);

    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn index_file_commits_large_files_in_multiple_chunks() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("large.jsonl");
    let mut body = String::new();
    for i in 0..(super::INDEX_CHUNK_SIZE + 3) {
        body.push_str(&format!(
            "{{\"sessionId\":\"s1\",\"content\":\"message {i}\"}}\n"
        ));
    }
    std::fs::write(&file, body).unwrap();

    let result = index_file(&pool, &file, "explicit_file").unwrap();

    assert_eq!(result.ingested, super::INDEX_CHUNK_SIZE + 3);
    assert_eq!(result.checkpoint_updates, 2);
}

#[test]
fn scanner_drops_oversized_metadata_fields() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("metadata.jsonl");
    let long_session = "s".repeat(super::MAX_SESSION_ID_LEN + 1);
    std::fs::write(
        &file,
        format!("{{\"sessionId\":\"{long_session}\",\"content\":\"hello\"}}\n"),
    )
    .unwrap();

    index_file(&pool, &file, "explicit_file").unwrap();
    let session: Option<String> = pool
        .get()
        .unwrap()
        .query_row("SELECT ai_session_id FROM logs LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(session, None);
}
```

- [ ] **Step 2: Run focused tests and confirm failure**

Run:

```bash
cargo test scanner::tests::index_file_rejects_oversized_jsonl_record_without_inserting_any_rows scanner::tests::index_file_commits_large_files_in_multiple_chunks scanner::tests::scanner_drops_oversized_metadata_fields
```

Expected: missing constants and behavior failures.

- [ ] **Step 3: Add constants and normalization helpers**

In `src/scanner.rs`, make these constants available to tests:

```rust
pub(crate) const INDEX_CHUNK_SIZE: usize = 500;
pub(crate) const MAX_RECORD_SIZE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SESSION_ID_LEN: usize = 128;
pub(crate) const MAX_PROJECT_LEN: usize = 512;
pub(crate) const MAX_TRANSCRIPT_PATH_LEN: usize = 1024;
```

Add helpers:

```rust
fn cap_string(value: Option<String>, max: usize) -> Option<String> {
    value.filter(|text| !text.is_empty() && text.len() <= max)
}

fn capped_transcript_path(path: &str) -> Option<String> {
    cap_string(Some(path.to_string()), MAX_TRANSCRIPT_PATH_LEN)
}
```

- [ ] **Step 4: Replace full-file read with `BufRead` streaming**

In `index_file`, replace:

```rust
let content = fs::read_to_string(&canonical_path)?;
let file_metadata = FileMetadata::from_path(&canonical_path, &content)?;
```

with:

```rust
let file = fs::File::open(&canonical_path)?;
let metadata = file.metadata()?;
let reader = std::io::BufReader::new(file);
let file_metadata = FileMetadata::from_metadata_and_path(&canonical_path, &metadata)?;
```

Then replace the `for (line_no, line) in content.lines().enumerate()` loop with:

```rust
use std::io::BufRead;

for (line_no, line_result) in reader.lines().enumerate() {
    let line = line_result?;
    if line.trim().is_empty() {
        continue;
    }
    if line.len() > MAX_RECORD_SIZE_BYTES {
        result.parse_errors += 1;
        continue;
    }
    if source_kind == SourceKind::CodexSession {
        fallback_project = codex::project_from_line(&line).or_else(|| fallback_project.clone());
    }
    match parse_line_for_source(source_kind, &line, &canonical_path, line_no) {
        Ok(Some(parsed)) => {
            let record_key = parsed.record_key;
            if existing_keys.contains(&record_key) || !seen_keys.insert(record_key.clone()) {
                result.skipped_dupes += 1;
                continue;
            }
            let message = scrub_ai_message(&parsed.message, None);
            let project = cap_string(
                parsed.ai_project.clone().or_else(|| fallback_project.clone()),
                MAX_PROJECT_LEN,
            );
            batch.push(LogBatchEntry {
                timestamp: parsed.timestamp.unwrap_or_else(|| {
                    chrono::Utc::now()
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string()
                }),
                hostname: "localhost".to_string(),
                facility: Some("transcript".to_string()),
                severity: "info".to_string(),
                app_name: Some(format!("{tool}-transcript")),
                process_id: None,
                raw: message.clone(),
                message,
                source_ip: format!("transcript://{}", source_kind.as_str()),
                docker_checkpoint: None,
                ai_tool: Some(tool.to_string()),
                ai_project: project,
                ai_session_id: cap_string(parsed.session_id, MAX_SESSION_ID_LEN),
                ai_transcript_path: capped_transcript_path(&canonical),
            });
            imports.push(record_key);
            if batch.len() >= INDEX_CHUNK_SIZE {
                commit_index_chunk(pool, source_id, &mut batch, &mut imports, &file_metadata, &mut result)?;
            }
        }
        Ok(None) => {}
        Err(error) => {
            result.parse_errors += 1;
            checkpoint_store.mark_error(source_id, &error.to_string())?;
        }
    }
}
```

- [ ] **Step 5: Add chunk commit helper**

Add below `index_file`:

```rust
fn commit_index_chunk(
    pool: &DbPool,
    source_id: i64,
    batch: &mut Vec<LogBatchEntry>,
    imports: &mut Vec<String>,
    file_metadata: &FileMetadata,
    result: &mut IndexResult,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    insert_logs_batch_in_tx(&tx, batch)?;
    checkpoint::record_imports_in_tx(&tx, source_id, imports, file_metadata)?;
    tx.commit()?;
    result.ingested += batch.len();
    result.checkpoint_updates += 1;
    batch.clear();
    imports.clear();
    Ok(())
}
```

Replace the final transaction block in `index_file` with:

```rust
commit_index_chunk(pool, source_id, &mut batch, &mut imports, &file_metadata, &mut result)?;
Ok(result)
```

- [ ] **Step 6: Update `FileMetadata`**

Replace `FileMetadata::from_path` with:

```rust
impl FileMetadata {
    fn from_metadata_and_path(path: &Path, metadata: &fs::Metadata) -> Result<Self> {
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64);
        Ok(Self {
            size: metadata.len(),
            mtime,
            content_hash: hash_text(&format!(
                "{}:{}:{}",
                path.display(),
                metadata.len(),
                mtime.unwrap_or_default()
            )),
        })
    }
}
```

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test scanner::tests::index_file_rejects_oversized_jsonl_record_without_inserting_any_rows scanner::tests::index_file_commits_large_files_in_multiple_chunks scanner::tests::scanner_drops_oversized_metadata_fields
```

Expected: all pass.

- [ ] **Step 8: Commit**

Run:

```bash
git add src/scanner.rs src/scanner_tests.rs
git commit -m "fix: stream transcript indexing in bounded chunks"
```

---

## Task 3: Enforce Storage Budget Before Transcript Chunk Commits

**Files:**
- Modify: `src/scanner.rs`
- Modify: `src/app/service.rs`
- Modify: `src/scanner_tests.rs`

- [ ] **Step 1: Write failing storage-block test**

Add to `src/scanner_tests.rs`:

```rust
#[test]
fn index_file_respects_storage_write_block_before_insert() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("session.jsonl");
    std::fs::write(&file, "{\"sessionId\":\"s1\",\"content\":\"should not insert\"}\n").unwrap();

    let mut storage = StorageConfig::for_test(dir.path().join("test.db"));
    storage.max_db_size_mb = 1;
    storage.recovery_db_size_mb = 1;
    storage.min_free_disk_mb = u64::MAX / 1024 / 1024;
    storage.recovery_free_disk_mb = u64::MAX / 1024 / 1024;

    let result = index_file_with_storage(&pool, &storage, &file, "explicit_file").unwrap();

    assert_eq!(result.ingested, 0);
    assert_eq!(result.storage_blocked_chunks, 1);
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
```

- [ ] **Step 2: Run test and confirm failure**

Run:

```bash
cargo test scanner::tests::index_file_respects_storage_write_block_before_insert
```

Expected: compile failure for missing `index_file_with_storage`.

- [ ] **Step 3: Add storage-aware scanner entrypoints**

In `src/scanner.rs`, add:

```rust
use crate::config::StorageConfig;
use crate::db::enforce_storage_budget;

pub fn index_roots_with_storage(
    pool: &DbPool,
    storage: &StorageConfig,
    root_override: Option<&Path>,
) -> Result<IndexResult> {
    index_roots_inner(pool, Some(storage), root_override)
}

pub fn index_file_with_storage(
    pool: &DbPool,
    storage: &StorageConfig,
    path: &Path,
    source_kind: &str,
) -> Result<IndexResult> {
    index_file_inner(pool, Some(storage), path, source_kind)
}
```

Refactor existing public functions to delegate:

```rust
pub fn index_roots(pool: &DbPool, root_override: Option<&Path>) -> Result<IndexResult> {
    index_roots_inner(pool, None, root_override)
}

pub fn index_file(pool: &DbPool, path: &Path, source_kind: &str) -> Result<IndexResult> {
    index_file_inner(pool, None, path, source_kind)
}
```

Rename current implementations to `index_roots_inner` and `index_file_inner`, adding `storage: Option<&StorageConfig>`.

- [ ] **Step 4: Gate chunk commits on storage budget**

Change `commit_index_chunk` signature:

```rust
fn commit_index_chunk(
    pool: &DbPool,
    storage: Option<&StorageConfig>,
    source_id: i64,
    batch: &mut Vec<LogBatchEntry>,
    imports: &mut Vec<String>,
    file_metadata: &FileMetadata,
    result: &mut IndexResult,
) -> Result<()>
```

At the top of `commit_index_chunk`, after the empty check, add:

```rust
if let Some(storage) = storage {
    let outcome = enforce_storage_budget(pool, storage)?;
    if outcome.write_blocked {
        result.storage_blocked_chunks += 1;
        return Ok(());
    }
}
```

Update both calls to `commit_index_chunk` to pass `storage`.

- [ ] **Step 5: Route service methods through storage-aware entrypoints**

In `src/app/service.rs`, replace `index_ai_roots` body with:

```rust
let storage = self.storage.clone();
self.run_db(move |pool| {
    scanner::index_roots_with_storage(
        pool,
        &storage,
        path.as_deref().map(std::path::Path::new),
    )
})
.await
.map_err(|error| match error {
    ServiceError::Internal(err) => ServiceError::InvalidInput(err.to_string()),
    other => other,
})
```

Replace `add_ai_file` body with:

```rust
let storage = self.storage.clone();
self.run_db(move |pool| {
    scanner::index_file_with_storage(
        pool,
        &storage,
        std::path::Path::new(&file),
        "explicit_file",
    )
})
.await
.map_err(|error| match error {
    ServiceError::Internal(err) => ServiceError::InvalidInput(err.to_string()),
    other => other,
})
```

- [ ] **Step 6: Run focused test**

Run:

```bash
cargo test scanner::tests::index_file_respects_storage_write_block_before_insert
```

Expected: pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/scanner.rs src/scanner_tests.rs src/app/service.rs
git commit -m "fix: enforce storage budget during transcript indexing"
```

---

## Task 4: Make Checkpoint Updates Transactional and Honest

**Files:**
- Modify: `src/scanner/checkpoint.rs`
- Modify: `src/scanner/checkpoint_tests.rs`
- Modify: `src/scanner.rs`
- Modify: `src/scanner_tests.rs`

- [ ] **Step 1: Write failing checkpoint timestamp tests**

Add to `src/scanner/checkpoint_tests.rs`:

```rust
#[test]
fn ensure_source_does_not_mark_source_indexed() {
    let (pool, _dir) = test_pool();
    let store = CheckpointStore::new(&pool);
    let source_id = store
        .ensure_source("/tmp/session.jsonl", "explicit_file")
        .unwrap();

    let row: (Option<String>, Option<String>) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT last_indexed_at, last_error FROM transcript_sources WHERE id = ?1",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(row, (None, None));
}

#[test]
fn mark_error_does_not_advance_last_indexed_at() {
    let (pool, _dir) = test_pool();
    let store = CheckpointStore::new(&pool);
    let source_id = store
        .ensure_source("/tmp/session.jsonl", "explicit_file")
        .unwrap();

    store.mark_error(source_id, "bad json").unwrap();

    let row: (Option<String>, Option<String>) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT last_indexed_at, last_error FROM transcript_sources WHERE id = ?1",
            [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(row.0, None);
    assert_eq!(row.1.as_deref(), Some("bad json"));
}
```

Add to `src/scanner_tests.rs`:

```rust
#[test]
fn failed_chunk_insert_does_not_record_import_identity() {
    let (pool, dir) = test_pool();
    let file = dir.path().join("bad-timestamp.jsonl");
    std::fs::write(
        &file,
        "{\"sessionId\":\"s1\",\"timestamp\":\"not-a-rfc3339\",\"content\":\"bad\"}\n",
    )
    .unwrap();

    let result = index_file(&pool, &file, "explicit_file");
    assert!(result.is_err());

    let imports: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM transcript_import_records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(imports, 0);
}
```

- [ ] **Step 2: Run tests and confirm failure**

Run:

```bash
cargo test scanner::checkpoint::tests::ensure_source_does_not_mark_source_indexed scanner::checkpoint::tests::mark_error_does_not_advance_last_indexed_at scanner::tests::failed_chunk_insert_does_not_record_import_identity
```

Expected: checkpoint timestamp tests fail against current `last_indexed_at` behavior.

- [ ] **Step 3: Change source creation and error update semantics**

In `src/scanner/checkpoint.rs`, replace the insert in `ensure_source` with:

```rust
conn.execute(
    "INSERT INTO transcript_sources (canonical_path, source_kind)
     VALUES (?1, ?2)",
    params![canonical_path, source_kind],
)?;
```

Replace `mark_error` update with:

```rust
conn.execute(
    "UPDATE transcript_sources
     SET last_error = ?2
     WHERE id = ?1",
    params![source_id, error],
)?;
```

- [ ] **Step 4: Validate transcript timestamps before DB insertion**

In `src/scanner.rs`, add:

```rust
fn normalize_timestamp(value: Option<String>) -> Result<String> {
    let timestamp = value.unwrap_or_else(|| {
        chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string()
    });
    chrono::DateTime::parse_from_rfc3339(&timestamp)
        .map_err(|error| anyhow::anyhow!("invalid transcript timestamp {timestamp}: {error}"))?;
    Ok(timestamp)
}
```

Replace the `timestamp:` expression in the `LogBatchEntry` construction with:

```rust
timestamp: normalize_timestamp(parsed.timestamp)?,
```

- [ ] **Step 5: Run checkpoint and scanner tests**

Run:

```bash
cargo test scanner::checkpoint scanner::tests::failed_chunk_insert_does_not_record_import_identity
```

Expected: pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add src/scanner/checkpoint.rs src/scanner/checkpoint_tests.rs src/scanner.rs src/scanner_tests.rs
git commit -m "fix: make transcript checkpoints advance only after imports"
```

---

## Task 5: Fix Claude and Codex Parser Fidelity

**Files:**
- Modify: `src/scanner.rs`
- Modify: `src/scanner/claude.rs`
- Modify: `src/scanner/codex.rs`
- Modify: `src/scanner/claude_tests.rs`
- Modify: `src/scanner/codex_tests.rs`
- Modify: `src/scanner_tests.rs`

- [ ] **Step 1: Write failing parser tests**

Add to `src/scanner/claude_tests.rs`:

```rust
#[test]
fn parse_line_joins_claude_content_object_arrays() {
    let line = r#"{"sessionId":"claude-array","message":{"content":[{"type":"text","text":"first"},{"type":"tool_result","content":"second"},{"type":"image","source":{"type":"base64"}}]}}"#;

    let parsed = parse_line(line, Path::new("/tmp/session.jsonl"), 0)
        .unwrap()
        .expect("object content array should produce a transcript record");

    assert_eq!(parsed.message, "first second");
    assert_eq!(parsed.session_id.as_deref(), Some("claude-array"));
}

#[test]
fn parse_line_extracts_claude_cwd_as_project() {
    let line = r#"{"sessionId":"claude-1","cwd":"/home/jmagar/workspace/syslog-mcp","content":"hello"}"#;

    let parsed = parse_line(line, Path::new("/tmp/session.jsonl"), 0)
        .unwrap()
        .expect("cwd record should produce a transcript record");

    assert_eq!(
        parsed.ai_project.as_deref(),
        Some("/home/jmagar/workspace/syslog-mcp")
    );
}
```

Add to `src/scanner_tests.rs`:

```rust
#[test]
fn codex_response_items_use_session_meta_id_not_item_id_as_session_id() {
    let (pool, dir) = test_pool();
    let codex_root = dir.path().join(".codex/sessions/2026/05/12");
    std::fs::create_dir_all(&codex_root).unwrap();
    let file = codex_root.join("rollout-realistic.jsonl");
    std::fs::write(
        &file,
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"real-session\",\"cwd\":\"/tmp/project\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"id\":\"item-1\",\"content\":[{\"type\":\"output_text\",\"text\":\"needle text\"}]},\"timestamp\":\"2026-05-12T00:00:00Z\"}\n"
        ),
    )
    .unwrap();

    index_file(&pool, &file, "codex_session").unwrap();
    let row: String = pool
        .get()
        .unwrap()
        .query_row("SELECT ai_session_id FROM logs LIMIT 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(row, "real-session");
}
```

- [ ] **Step 2: Run parser tests and confirm failure**

Run:

```bash
cargo test scanner::claude::tests::parse_line_joins_claude_content_object_arrays scanner::claude::tests::parse_line_extracts_claude_cwd_as_project scanner::tests::codex_response_items_use_session_meta_id_not_item_id_as_session_id
```

Expected: failures for Claude object arrays/project extraction and Codex session id.

- [ ] **Step 3: Extend Claude parser**

In `src/scanner/claude.rs`, replace `extract_message` with:

```rust
fn extract_message(value: &Value) -> String {
    for pointer in ["/content", "/message", "/message/content"] {
        if let Some(text) = value.pointer(pointer).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    for pointer in ["/content", "/message/content"] {
        if let Some(items) = value.pointer(pointer).and_then(Value::as_array) {
            let text = join_content_items(items);
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn join_content_items(items: &[Value]) -> String {
    items
        .iter()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("text").and_then(Value::as_str))
                .or_else(|| item.get("content").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_project(value: &Value) -> Option<String> {
    value
        .get("cwd")
        .or_else(|| value.get("projectPath"))
        .or_else(|| value.get("project_path"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
```

In `parse_line`, set:

```rust
ai_project: extract_project(&value),
```

- [ ] **Step 4: Add Codex file context**

In `src/scanner.rs`, add:

```rust
#[derive(Debug, Clone, Default)]
struct TranscriptFileContext {
    session_id: Option<String>,
    project: Option<String>,
}
```

Before the line loop in `index_file_inner`, add:

```rust
let mut file_context = TranscriptFileContext::default();
```

Replace the Codex metadata update with:

```rust
if source_kind == SourceKind::CodexSession {
    if let Some(meta) = codex::metadata_from_line(&line) {
        file_context.session_id = meta.session_id.or(file_context.session_id);
        file_context.project = meta.project.or(file_context.project);
    }
    fallback_project = file_context.project.clone().or_else(|| fallback_project.clone());
}
```

When computing `project`, prefer parsed project then file context then fallback:

```rust
let project = cap_string(
    parsed
        .ai_project
        .clone()
        .or_else(|| file_context.project.clone())
        .or_else(|| fallback_project.clone()),
    MAX_PROJECT_LEN,
);
```

When computing session id, prefer file context for Codex:

```rust
let session_id = if source_kind == SourceKind::CodexSession {
    file_context.session_id.clone().or(parsed.session_id)
} else {
    parsed.session_id
};
```

Use `session_id` in the `LogBatchEntry`.

- [ ] **Step 5: Add Codex metadata extractor**

In `src/scanner/codex.rs`, add:

```rust
#[derive(Debug, Clone, Default)]
pub struct CodexLineMetadata {
    pub session_id: Option<String>,
    pub project: Option<String>,
}

pub fn metadata_from_line(line: &str) -> Option<CodexLineMetadata> {
    let value: Value = serde_json::from_str(line).ok()?;
    let payload = value.get("payload").unwrap_or(&value);
    let session_id = if value.get("type").and_then(Value::as_str) == Some("session_meta") {
        payload.get("id").and_then(Value::as_str).map(ToString::to_string)
    } else {
        None
    };
    let project = extract_project(&value);
    if session_id.is_none() && project.is_none() {
        None
    } else {
        Some(CodexLineMetadata { session_id, project })
    }
}
```

In `parse_line`, change the fallback session id to only use file stem:

```rust
let session_id = value
    .get("sessionId")
    .or_else(|| value.get("session_id"))
    .or_else(|| value.pointer("/session/id"))
    .and_then(Value::as_str)
    .map(ToString::to_string)
    .or_else(|| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToString::to_string)
    });
```

Do not use `payload.id` as session id for response items.

- [ ] **Step 6: Run parser tests**

Run:

```bash
cargo test scanner::claude scanner::codex scanner::tests::codex_response_items_use_session_meta_id_not_item_id_as_session_id
```

Expected: pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/scanner.rs src/scanner/claude.rs src/scanner/codex.rs src/scanner/claude_tests.rs src/scanner/codex_tests.rs src/scanner_tests.rs
git commit -m "fix: parse realistic Claude and Codex transcripts"
```

---

## Task 6: Bound AI Analytics and Expose Complete Truncation Metadata

**Files:**
- Modify: `src/db/models.rs`
- Modify: `src/app/models.rs`
- Modify: `src/db/queries.rs`
- Modify: `src/db/analytics.rs`
- Modify: `src/db/queries_tests.rs`
- Modify: `src/db/analytics_tests.rs`

- [ ] **Step 1: Write failing truncation and snippet tests**

Add to `src/db/queries_tests.rs`:

```rust
#[test]
fn list_ai_tools_reports_truncation_when_more_than_limit_exists() {
    let (pool, _dir) = test_pool();
    let entries: Vec<_> = (0..105)
        .map(|i| make_ai_entry(
            "2026-01-01T00:00:00Z",
            "host-a",
            &format!("tool-{i:03}"),
            "/tmp/project",
            &format!("sess-{i:03}"),
            "message",
        ))
        .collect();
    insert_logs_batch(&pool, &entries).unwrap();

    let result = list_ai_tools(&pool, &ListAiToolsParams::default()).unwrap();

    assert_eq!(result.tools.len(), 100);
    assert!(result.truncated);
    assert_eq!(result.total_tools, 105);
}
```

Add to `src/db/analytics_tests.rs`:

```rust
#[test]
fn project_context_representative_entries_are_snippet_bounded() {
    let (pool, _d) = test_pool();
    insert_logs_batch(
        &pool,
        &[ai_entry(
            "2026-01-01T00:00:00Z",
            "claude",
            "/tmp/project",
            "sess-1",
            &"x".repeat(600),
        )],
    )
    .unwrap();

    let result = get_ai_project_context(
        &pool,
        &AiProjectContextParams {
            project: "/tmp/project".into(),
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(result.recent_entries.len(), 1);
    assert!(result.recent_entries[0].message.len() <= 256);
}
```

- [ ] **Step 2: Run tests and confirm failure**

Run:

```bash
cargo test db::queries::tests::list_ai_tools_reports_truncation_when_more_than_limit_exists db::analytics::tests::project_context_representative_entries_are_snippet_bounded
```

Expected: missing fields and oversized message failure.

- [ ] **Step 3: Add truncation metadata to DB/app models**

In `src/db/models.rs`, change inventory result structs to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiToolsResult {
    pub total_tools: usize,
    pub truncated: bool,
    pub tools: Vec<AiToolInventoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiProjectsResult {
    pub total_projects: usize,
    pub truncated: bool,
    pub projects: Vec<AiProjectInventoryEntry>,
}
```

In `src/app/models.rs`, mirror those fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiToolsResponse {
    pub total_tools: usize,
    pub truncated: bool,
    pub tools: Vec<AiToolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAiProjectsResponse {
    pub total_projects: usize,
    pub truncated: bool,
    pub projects: Vec<AiProjectEntry>,
}
```

Update the `From` impls to copy the new fields.

- [ ] **Step 4: Fetch one extra inventory row and compute totals**

In `list_ai_tools`, set:

```rust
const LIMIT: usize = 100;
```

Change the SQL suffix to:

```rust
sql.push_str(&format!(" GROUP BY ai_tool ORDER BY event_count DESC, ai_tool ASC LIMIT {}", LIMIT + 1));
```

After collecting rows:

```rust
let total_tools = tools.len();
let truncated = total_tools > LIMIT;
let tools = tools.into_iter().take(LIMIT).collect();
Ok(ListAiToolsResult {
    total_tools,
    truncated,
    tools,
})
```

Apply the same pattern to `list_ai_projects` with `LIMIT: usize = 200`, `total_projects`, and `truncated`.

- [ ] **Step 5: Bound project-context representative messages**

In `src/db/analytics.rs`, add:

```rust
fn truncate_message(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let prefix: String = value.chars().take(max_chars - 1).collect();
    format!("{prefix}…")
}
```

After collecting `recent_entries`, map them:

```rust
let recent_entries = recent_entries
    .into_iter()
    .map(|mut entry| {
        entry.message = truncate_message(entry.message, 256);
        entry.raw = String::new();
        entry
    })
    .collect();
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test db::queries::tests::list_ai_tools_reports_truncation_when_more_than_limit_exists db::analytics::tests::project_context_representative_entries_are_snippet_bounded app::service::tests::ai_service_methods_return_seeded_data
```

Expected: pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/db/models.rs src/app/models.rs src/db/queries.rs src/db/analytics.rs src/db/queries_tests.rs src/db/analytics_tests.rs
git commit -m "fix: bound AI analytics result metadata"
```

---

## Task 7: Tighten OTLP AI Metadata Contract

**Files:**
- Modify: `src/otlp.rs`
- Modify: `src/otlp_tests.rs`
- Modify: `docs/mcp/TOOLS.md`

- [ ] **Step 1: Write failing tests for oversized project/session fields**

Add to `src/otlp_tests.rs`:

```rust
#[test]
fn build_entries_ignores_oversized_ai_project_and_session_id() {
    let peer = "127.0.0.1:1".parse().unwrap();
    let req = request_with_log_attrs(vec![
        kv("ai.tool", av_string("claude")),
        kv("session.id", av_string(&"s".repeat(129))),
        kv("project.path", av_string(&"p".repeat(513))),
    ]);

    let entries = build_entries(&req, peer);

    assert_eq!(entries[0].ai_tool.as_deref(), Some("claude"));
    assert_eq!(entries[0].ai_session_id, None);
    assert_eq!(entries[0].ai_project, None);
}
```

Add helper near existing OTLP test builders:

```rust
fn request_with_log_attrs(attrs: Vec<KeyValue>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("host.name", av_string("nashost"))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: vec![LogRecord {
                    time_unix_nano: 0,
                    observed_time_unix_nano: 0,
                    severity_number: 9,
                    severity_text: String::new(),
                    body: Some(av_string("msg")),
                    attributes: attrs,
                    dropped_attributes_count: 0,
                    flags: 0,
                    trace_id: vec![],
                    span_id: vec![],
                    event_name: String::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}
```

- [ ] **Step 2: Run test**

Run:

```bash
cargo test otlp::tests::build_entries_ignores_oversized_ai_project_and_session_id
```

Expected: pass if current behavior is already correct; keep the test as coverage.

- [ ] **Step 3: Centralize OTLP caps if needed**

If the test fails, add constants in `src/otlp.rs`:

```rust
const MAX_AI_TOOL_LEN: usize = 64;
const MAX_AI_SESSION_ID_LEN: usize = 128;
const MAX_AI_PROJECT_LEN: usize = 512;
```

Use them in the existing `.filter(|value| value.len() <= ...)` calls.

- [ ] **Step 4: Document producer-supplied trust model**

In `docs/mcp/TOOLS.md`, add this paragraph under Transcript Visibility Policy:

```markdown
OTLP AI attributes (`ai.tool`, `ai_tool`, `session.id`, `session_id`, `project.path`, `codebase.root_path`, and `session.cwd`) are producer-supplied metadata accepted from configured trusted emitters. They are used only for search and grouping; they are not authentication or authorization signals. Unknown AI tools and oversized AI project/session values are ignored before storage.
```

- [ ] **Step 5: Commit**

Run:

```bash
git add src/otlp.rs src/otlp_tests.rs docs/mcp/TOOLS.md
git commit -m "test: cover OTLP AI metadata trust bounds"
```

---

## Task 8: Fix MCP Schema, Help, and Documentation Drift

**Files:**
- Modify: `src/mcp/schemas.rs`
- Modify: `src/mcp/tools.rs`
- Modify: `docs/CLI.md`
- Modify: `docs/mcp/TOOLS.md`
- Modify: `docs/mcp/SCHEMA.md`
- Modify: `docs/mcp/TESTS.md`
- Modify: `README.md`
- Modify: `docs/expansion.md`

- [ ] **Step 1: Write schema/help regression test**

Add to `src/mcp/tools_tests.rs`:

```rust
#[test]
fn ai_action_schema_descriptions_cover_search_sessions_query() {
    let defs = super::schemas::tool_definitions();
    let schema = &defs[0]["inputSchema"]["properties"];
    let query = schema["query"]["description"].as_str().unwrap();
    assert!(query.contains("search_sessions"), "query schema must mention search_sessions");
}
```

- [ ] **Step 2: Run test and confirm failure**

Run:

```bash
cargo test mcp::tools::tests::ai_action_schema_descriptions_cover_search_sessions_query
```

Expected: failure because current query description mentions only `search` and `correlate`.

- [ ] **Step 3: Update `src/mcp/schemas.rs` descriptions**

Change the `query` property description to:

```rust
"description": "For action=search, action=search_sessions, or action=correlate: FTS5 query. Examples: 'kernel panic', 'OOM AND killer', '\"connection refused\"', 'error*'. For hyphenated transcript terms, use phrase syntax such as '\"smoke-test\"'."
```

Update the `limit` property description to include inventory truncation metadata:

```rust
"description": "For action=search: max results, default 100, max 1000. For action=sessions: max results, default 100, max 1000. For action=search_sessions: max grouped sessions, default 20, max 100. For action=project_context: recent representative entries, default 5, max 20. For action=list_ai_tools and list_ai_projects: fixed server caps return truncation metadata. For action=correlate: max total events, default 500, max 999."
```

- [ ] **Step 4: Update MCP help text**

In `src/mcp/tools.rs`, update the `search_sessions`, `usage_blocks`, `project_context`, and Transcript Visibility sections so they state:

```markdown
`search_sessions` groups AI transcript rows by project/tool/session/host and ranks groups with SQLite FTS5 score plus deterministic recency tie-breakers. It is not a replacement for flat `search`.
```

```markdown
AI transcript rows imported through `syslog ai index` or `syslog ai add` are stored in `logs`. They are visible through `search`, `tail`, `context`, and `get`. Transcript messages are passed through the existing AI secret scrubber before storage, so known token patterns are redacted, but local paths in `ai_transcript_path` remain visible.
```

- [ ] **Step 5: Update docs**

Apply these exact documentation facts:

- `docs/CLI.md`: under `syslog ai index`, add "Bare `syslog ai index` scans only `~/.claude/projects` and `~/.codex/sessions`. `--path` must be a known transcript root, a child of a known transcript root, or one `.jsonl` file; broad paths are rejected before scanning."
- `docs/CLI.md`: under `syslog ai add`, add "The command is rerunnable. Existing import identities are skipped and reported as `skipped_dupes`."
- `docs/mcp/TOOLS.md`: replace "no redaction is applied automatically" with "transcript message text is scrubbed before storage; transcript paths are not redacted."
- `README.md`: add an "AI Session Indexing" subsection that names the CLI commands, idempotence behavior, storage-budget blocking, and raw visibility policy.
- `docs/expansion.md`: change the stale transcript row from `❌ missing` to "implemented for CLI indexing and metadata queries; operational hardening tracked in `docs/superpowers/plans/2026-05-12-mnemo-fully-operational.md`."
- `docs/mcp/TESTS.md`: list seeded AI transcript smoke checks for `search_sessions`, `usage_blocks`, `project_context`, `list_ai_tools`, and `list_ai_projects`.
- `docs/mcp/SCHEMA.md`: update the action list/argument descriptions to match `src/mcp/schemas.rs`.

- [ ] **Step 6: Run schema/help/docs checks**

Run:

```bash
cargo test mcp::tools::tests::ai_action_schema_descriptions_cover_search_sessions_query mcp::tools::tests::public_action_references_cover_schema_registry
rg -n "no redaction is applied automatically|claude/codex `.jsonl` transcripts \\| none \\| ❌ missing" docs README.md
```

Expected: tests pass and `rg` returns no stale redaction/missing-transcript lines.

- [ ] **Step 7: Commit**

Run:

```bash
git add src/mcp/schemas.rs src/mcp/tools.rs src/mcp/tools_tests.rs docs/CLI.md docs/mcp/TOOLS.md docs/mcp/SCHEMA.md docs/mcp/TESTS.md README.md docs/expansion.md
git commit -m "docs: clarify AI transcript operational semantics"
```

---

## Task 9: Add Seeded CLI, HTTP MCP, and mcporter Verification

**Files:**
- Modify: `scripts/smoke-test.sh`
- Modify: `tests/test_live.sh`
- Modify: `tests/mcporter/test-tools.sh`
- Create: `tests/fixtures/ai-session-smoke.jsonl`

- [ ] **Step 1: Add a deterministic fixture**

Create `tests/fixtures/ai-session-smoke.jsonl`:

```jsonl
{"sessionId":"ai-smoke-session","timestamp":"2026-05-12T18:00:00Z","content":"authentication ai-smoke first message"}
{"sessionId":"ai-smoke-session","timestamp":"2026-05-12T18:01:00Z","content":"authentication ai-smoke second message"}
```

- [ ] **Step 2: Seed fixture in `scripts/smoke-test.sh`**

Before `Action: AI session analytics`, add:

```bash
echo "Seeding AI transcript fixture"
SYSLOG_MCP_DB_PATH="${SYSLOG_MCP_DB_PATH:-data/syslog.db}" \
  cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json >/tmp/syslog-ai-smoke-seed.json
python3 - <<'PY'
import json
with open('/tmp/syslog-ai-smoke-seed.json') as f:
    data = json.load(f)
assert data["ingested"] >= 0
assert data["file_errors"] == []
PY
```

Change the `search_sessions` call query from `authentication` to:

```bash
SEARCH_SESSIONS=$(mcp_call search_sessions "query=ai-smoke" "limit=10" 2>&1)
```

Add this assertion:

```bash
SEARCH_SESSIONS_COUNT=$(echo "$SEARCH_SESSIONS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)['sessions']))" 2>/dev/null || echo "0")
assert_gte "search_sessions: seeded result returned" "$SEARCH_SESSIONS_COUNT" 1
```

- [ ] **Step 3: Seed fixture in `tests/test_live.sh`**

In the setup phase after the server is healthy and before `suite_sessions`, add:

```bash
seed_ai_fixture() {
  SYSLOG_MCP_DB_PATH="${SYSLOG_MCP_DB_PATH:-${DB_PATH:-data/syslog.db}}" \
    cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json >/tmp/syslog-test-live-ai-seed.json
  assert_jq "seed AI fixture — no file errors" "$(cat /tmp/syslog-test-live-ai-seed.json)" '.file_errors | length' "0"
}
```

Call `seed_ai_fixture` before the sessions suite.

Change the live search query to:

```bash
search_sessions_result="$(call_tool syslog '{"action":"search_sessions","query":"ai-smoke","limit":10}')" || search_sessions_result=""
assert_jq "syslog search_sessions — seeded sessions present" "${search_sessions_result}" '.sessions | length >= 1'
```

- [ ] **Step 4: Seed fixture in `tests/mcporter/test-tools.sh`**

Before `suite_sessions`, add:

```bash
seed_ai_fixture() {
  SYSLOG_MCP_DB_PATH="${SYSLOG_MCP_DB_PATH:-data/syslog.db}" \
    cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json >/tmp/syslog-mcporter-ai-seed.json
}
```

Call `seed_ai_fixture`.

Change the `search_sessions` test query to:

```bash
run_test "syslog search_sessions: returns seeded sessions array" \
  syslog search_sessions '{"query":"ai-smoke","limit":10}' "sessions"
```

- [ ] **Step 5: Run seeded CLI and HTTP smoke locally**

Run:

```bash
tmpdir=$(mktemp -d)
SYSLOG_MCP_DB_PATH="$tmpdir/syslog.db" cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json
SYSLOG_MCP_DB_PATH="$tmpdir/syslog.db" cargo run --quiet -- ai search ai-smoke --json
rm -rf "$tmpdir"
```

Expected: `ai add` reports either `ingested: 2` or `skipped_dupes: 2`; `ai search` returns at least one session.

- [ ] **Step 6: Run full smoke if a server is available**

Run:

```bash
bash scripts/smoke-test.sh
```

Expected: all checks pass, including "search_sessions: seeded result returned".

- [ ] **Step 7: Commit**

Run:

```bash
git add tests/fixtures/ai-session-smoke.jsonl scripts/smoke-test.sh tests/test_live.sh tests/mcporter/test-tools.sh
git commit -m "test: seed AI transcript live smoke coverage"
```

---

## Task 10: Final Verification, Versioning, and Push

**Files:**
- Modify: version-bearing files through `scripts/bump-version.sh`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Run full local verification**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
bash scripts/check-version-sync.sh
```

Expected: all pass.

- [ ] **Step 2: Run operational CLI smoke with a temp DB**

Run:

```bash
tmpdir=$(mktemp -d)
export SYSLOG_MCP_DB_PATH="$tmpdir/syslog.db"
cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json
cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json
cargo run --quiet -- ai search ai-smoke --json
cargo run --quiet -- ai blocks --json
cargo run --quiet -- ai context --project "$(pwd)" --json
cargo run --quiet -- ai tools --json
cargo run --quiet -- ai projects --json
rm -rf "$tmpdir"
```

Expected:

- First `ai add`: `ingested` is `2`.
- Second `ai add`: `ingested` is `0` and `skipped_dupes` is `2`.
- `ai search`: at least one session for `ai-smoke`.
- `ai tools`: includes `claude`.
- No command returns `file_errors`.

- [ ] **Step 3: Run HTTP MCP runtime smoke**

Run:

```bash
tmpdir=$(mktemp -d)
export SYSLOG_MCP_DB_PATH="$tmpdir/syslog.db"
export SYSLOG_HOST=127.0.0.1
export SYSLOG_PORT=15140
export SYSLOG_MCP_HOST=127.0.0.1
export SYSLOG_MCP_PORT=33100
cargo run --quiet -- ai add --file tests/fixtures/ai-session-smoke.jsonl --json
cargo run --quiet -- serve mcp >"$tmpdir/server.log" 2>&1 &
pid=$!
for i in $(seq 1 50); do curl -sf http://127.0.0.1:33100/health >/dev/null && break; sleep 0.1; done
curl -sf -X POST http://127.0.0.1:33100/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"syslog","arguments":{"action":"search_sessions","query":"ai-smoke","limit":5}}}' \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); text=d["result"]["content"][0]["text"]; assert json.loads(text)["sessions"]'
kill "$pid"
wait "$pid" 2>/dev/null || true
rm -rf "$tmpdir"
```

Expected: command exits `0`; MCP returns at least one AI session.

- [ ] **Step 4: Bump version and changelog**

Because this completes an existing feature rather than adding a new public namespace, use a patch bump unless the final branch also changes response shapes incompatibly. Run:

```bash
bash scripts/bump-version.sh patch
```

Add a `CHANGELOG.md` entry under the new version with these bullets:

```markdown
- Hardened `syslog ai index` and `syslog ai add` with explicit path safety, streaming JSONL parsing, storage-budget checks, and transactional checkpoint updates.
- Added seeded AI transcript smoke coverage for CLI, HTTP MCP, and mcporter paths.
- Clarified AI transcript raw visibility, secret scrubbing, and OTLP producer-supplied metadata semantics in docs.
```

- [ ] **Step 5: Final status, commit, and push**

Run:

```bash
git status --short
git add .
git commit -m "fix: make mnemo AI session port fully operational"
git push
```

Expected: push succeeds and `git status --short` is clean afterward.

---

## Self-Review

Spec coverage:

- Scanner path safety, default roots, symlink rejection, unsupported files, deterministic traversal, chunking, max file/record size, duplicate prevention, storage budget, checkpoint transactions, parser fidelity, secret scrubbing, docs, smoke tests, and final operational probes are each assigned to a task.
- Existing `syslog sessions` and MCP `action=sessions` compatibility remain untouched; tasks harden the feature around the existing public surfaces.
- New analytics remain in `src/db/queries.rs`, `src/db/analytics.rs`, and `src/app/service.rs`; no unnecessary `db/ai.rs` or `app/ai.rs` extraction is introduced.

Placeholder scan:

- The plan contains concrete paths, test names, commands, expected outcomes, and code snippets for each implementation step.
- No step asks an implementer to "add validation" or "write tests" without naming exact behavior and expected assertions.

Type consistency:

- Scanner entrypoints are `index_roots_with_storage`, `index_file_with_storage`, `index_roots_inner`, and `index_file_inner`.
- Result fields added in Task 1 are used consistently by CLI output and later storage tests.
- Inventory metadata fields are `total_tools`, `total_projects`, and `truncated`, matching the DB and app model names.

Plan complete and saved to `docs/superpowers/plans/2026-05-12-mnemo-fully-operational.md`. Two execution options:

1. Subagent-Driven (recommended) - dispatch a fresh subagent per task, review between tasks, fast iteration.
2. Inline Execution - execute tasks in this session using executing-plans, batch execution with checkpoints.
