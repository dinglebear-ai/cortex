use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::ai_project::normalize_local_ai_project_path;
use crate::config::StorageConfig;
use crate::db::{
    DbPool, HookEventInsert, LogBatchEntry, McpEventInsert, SkillEventInsert,
    enforce_storage_budget, insert_hook_events_in_tx, insert_logs_batch_in_tx,
    insert_mcp_events_in_tx, insert_skill_events_in_tx,
};
use crate::ingest_metadata::bounded_metadata_json;
use crate::receiver::enrichment::{project_from_transcript_path, scrub_ai_message};
use crate::scanner::hook_events::extract_claude_hook_events;
use crate::scanner::mcp_events::{extract_claude_mcp_events, extract_codex_mcp_events};
use crate::scanner::skill_events::{extract_claude_skill_events, extract_codex_skill_events};

mod checkpoint;
mod claude;
mod codex;
pub(crate) mod gemini;
pub(crate) mod hook_events;
pub(crate) mod mcp_events;
pub(crate) mod skill_events;

pub use checkpoint::CheckpointStore;

const MAX_FILE_SIZE_BYTES: u64 = 1024 * 1024 * 1024;
#[cfg(not(test))]
const MAX_RECORD_SIZE_BYTES: usize = 32 * 1024 * 1024;
#[cfg(test)]
const MAX_RECORD_SIZE_BYTES: usize = 16 * 1024 * 1024;
const MAX_INDEX_CHUNK_RECORDS: usize = 500;
const MAX_INDEX_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_AI_PROJECT_CHARS: usize = 512;
const MAX_AI_SESSION_ID_CHARS: usize = 128;
const MAX_TRANSCRIPT_PATH_CHARS: usize = 1024;

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
    pub dropped_metadata_fields: usize,
    pub checkpoint_updates: usize,
    pub file_errors: Vec<IndexFileError>,
    #[serde(skip)]
    dropped_metadata_field_keys: HashSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct IndexOptions {
    pub root_override: Option<PathBuf>,
    pub force: bool,
    pub since_mtime_nanos: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct IndexFileOptions {
    pub force: bool,
}

/// Raw skill-extraction source paired 1:1 with each `LogBatchEntry` pushed
/// into a chunk's `batch` vector. Carried alongside `batch`/`imports` because
/// skill extraction needs the PRE-SCRUB parsed value (Claude JSON) or raw
/// extracted text (Codex), not the already-scrubbed `LogBatchEntry.message`.
///
/// Eng review Fix 1: `Claude` wraps the `serde_json::Value` that
/// `ParsedTranscriptRecord.raw_value` already carries (Task 2) — NOT a
/// re-parse of `line_text`. `claude::parse_line` parses the line's JSON
/// exactly once; this side channel just moves that already-parsed value
/// forward instead of throwing it away and parsing again.
#[derive(Debug, Clone)]
enum ChunkSkillSource {
    Claude(serde_json::Value),
    Codex(String),
    None,
}

/// Raw MCP-extraction source paired 1:1 with each `LogBatchEntry` pushed
/// into a chunk's `batch` vector, mirroring `ChunkSkillSource` above. Both
/// Claude and Codex carry `raw_value` on every record now (see
/// `codex::parse_line`'s doc comment); the clone is gated behind a cheap
/// substring pre-check on `line_text` (`tool_use`/`tool_result` for Claude,
/// `function_call` for Codex — the exact literals
/// `extract_claude_mcp_events`/`extract_codex_mcp_events` themselves match
/// on), so a transcript line that can never produce an MCP event never
/// pays the clone cost.
#[derive(Debug, Clone)]
enum ChunkMcpSource {
    Claude(serde_json::Value),
    Codex(serde_json::Value),
    None,
}

#[derive(Debug, Clone, Default)]
pub struct CheckpointListOptions {
    pub errors_only: bool,
    pub missing_only: bool,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointEntry {
    pub canonical_path: String,
    pub source_kind: String,
    pub file_size: Option<i64>,
    pub file_mtime: Option<i64>,
    pub content_hash: Option<String>,
    pub last_offset: Option<i64>,
    pub last_indexed_at: Option<String>,
    pub last_error: Option<String>,
    pub imported_records: i64,
    pub missing: bool,
    pub parse_errors: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexFileError {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParseErrorListOptions {
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParseErrorEntry {
    pub canonical_path: String,
    pub source_kind: String,
    pub line_no: i64,
    pub error: String,
    pub record_preview: Option<String>,
    pub seen_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct PruneCheckpointsOptions {
    pub missing_only: bool,
    pub dry_run: bool,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PruneCheckpointsResult {
    pub matched: usize,
    pub pruned: usize,
    pub dry_run: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiDoctorReport {
    pub db_path: String,
    pub db_schema_version: i64,
    pub db_last_migration_at: Option<String>,
    pub known_schema_version: i64,
    pub schema_current: bool,
    pub claude_root: TranscriptRootStatus,
    pub codex_root: TranscriptRootStatus,
    pub gemini_root: TranscriptRootStatus,
    pub checkpoint_count: i64,
    pub checkpoint_error_count: i64,
    pub missing_checkpoint_count: i64,
    pub imported_record_count: i64,
    pub parse_error_count: i64,
    pub newest_indexed_path: Option<String>,
    pub newest_indexed_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchemaDriftMigration {
    pub version: i64,
    pub applied_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiIndexingHealth {
    pub db_schema_version: i64,
    pub db_last_migration_at: Option<String>,
    pub known_schema_version: i64,
    pub schema_current: bool,
    pub schema_drift_detected: bool,
    pub schema_drift_migrations: Vec<SchemaDriftMigration>,
    pub last_successful_ingest_at: Option<String>,
    pub recent_failure_count: i64,
    pub first_failure_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub affected_paths: Vec<String>,
    pub recent_schema_error_count: i64,
    pub stale_indicators: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptRootStatus {
    pub path: String,
    pub exists: bool,
    pub readable: bool,
    pub writable: bool,
    pub owner_uid: Option<u32>,
    pub owner_gid: Option<u32>,
    pub mode: Option<u32>,
    pub strict_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    ClaudeProject,
    CodexSession,
    GeminiSession,
    ExplicitFile,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeProject => "claude_project",
            Self::CodexSession => "codex_session",
            Self::GeminiSession => "gemini_session",
            Self::ExplicitFile => "explicit_file",
        }
    }
}

pub fn validate_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(PathScanError::SymlinkNotAllowed(path.to_path_buf()).into());
    }
    if metadata.is_file() && metadata.len() > MAX_FILE_SIZE_BYTES {
        return Err(PathScanError::FileTooLarge(path.to_path_buf()).into());
    }
    Ok(())
}

pub fn is_supported_transcript_file(path: &Path) -> bool {
    supported_discovered_file(path)
}

pub(crate) fn should_descend_transcript_dir(path: &Path) -> bool {
    !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".cache" | ".git" | "node_modules" | "target")
    )
}

pub fn is_invalid_input_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PathScanError>().is_some()
        || error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
}

pub fn default_transcript_roots() -> Vec<PathBuf> {
    default_roots()
}

pub fn validate_transcript_scan_path(path: &Path) -> Result<PathBuf> {
    validate_path(path)?;
    reject_broad_scan_path(path)?;
    Ok(path.canonicalize()?)
}

fn classify_path_error(error: &anyhow::Error, result: &mut IndexResult) {
    if let Some(path_error) = error.downcast_ref::<PathScanError>() {
        match path_error {
            PathScanError::SymlinkNotAllowed(_) => result.skipped_symlinks += 1,
            PathScanError::UnsafePath(_) => result.skipped_unsafe_paths += 1,
            PathScanError::FileTooLarge(_) | PathScanError::ExpectedFile(_) => {}
        }
    }
}

fn reject_broad_scan_path(path: &Path) -> Result<()> {
    let canonical = path.canonicalize()?;
    let home = crate::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok());
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    if canonical == Path::new("/")
        || home.as_ref().is_some_and(|value| &canonical == value)
        || cwd.as_ref().is_some_and(|value| &canonical == value)
    {
        return Err(PathScanError::UnsafePath(canonical).into());
    }
    if canonical.is_dir() && !is_known_transcript_root(&canonical) && !test_temp_path(&canonical) {
        return Err(PathScanError::UnsafePath(canonical).into());
    }
    if canonical.is_file() && !supported_discovered_file(&canonical) {
        return Err(PathScanError::UnsafePath(canonical).into());
    }
    Ok(())
}

#[derive(Debug)]
enum PathScanError {
    SymlinkNotAllowed(PathBuf),
    UnsafePath(PathBuf),
    FileTooLarge(PathBuf),
    ExpectedFile(PathBuf),
}

impl std::fmt::Display for PathScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SymlinkNotAllowed(path) => {
                write!(f, "symlinks are not allowed: {}", path.display())
            }
            Self::UnsafePath(path) => write!(
                f,
                "unsafe transcript scan path: {}; pass a known transcript root or one supported transcript file",
                path.display()
            ),
            Self::FileTooLarge(path) => write!(f, "file exceeds max size: {}", path.display()),
            Self::ExpectedFile(path) => write!(f, "expected a file path: {}", path.display()),
        }
    }
}

impl std::error::Error for PathScanError {}

fn is_known_transcript_root(path: &Path) -> bool {
    let Some(home) = crate::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    // macOS commonly exposes /tmp through the /private/tmp symlink. Compare
    // canonical roots to the already-canonical candidate so a disposable HOME
    // beneath /tmp remains a recognized transcript boundary.
    let home = home.canonicalize().unwrap_or(home);
    let allowed = [
        home.join(".claude/projects"),
        home.join(".codex/sessions"),
        home.join(".codex/worktrees"),
        home.join(".gemini/tmp"),
    ];
    allowed.iter().any(|root| {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        path == root || path.starts_with(root)
    })
}

fn test_temp_path(path: &Path) -> bool {
    if !cfg!(test) {
        return false;
    }
    let temp_dir = std::env::temp_dir();
    let canonical_temp = temp_dir
        .canonicalize()
        .unwrap_or_else(|_| temp_dir.to_path_buf());
    path.starts_with(canonical_temp)
}

pub fn index_roots(pool: &DbPool, root_override: Option<&Path>) -> Result<IndexResult> {
    index_roots_with_storage(pool, root_override, None)
}

pub fn index_roots_with_storage(
    pool: &DbPool,
    root_override: Option<&Path>,
    storage: Option<&StorageConfig>,
) -> Result<IndexResult> {
    index_roots_with_options(
        pool,
        IndexOptions {
            root_override: root_override.map(Path::to_path_buf),
            ..Default::default()
        },
        storage,
    )
}

pub fn index_roots_with_options(
    pool: &DbPool,
    options: IndexOptions,
    storage: Option<&StorageConfig>,
) -> Result<IndexResult> {
    let roots = match options.root_override.as_deref() {
        Some(path) => {
            let mut result = IndexResult::default();
            match validate_transcript_scan_path(path) {
                Ok(canonical) => vec![canonical],
                Err(error) => {
                    classify_path_error(&error, &mut result);
                    record_file_error(&mut result, path, &error);
                    return Ok(result);
                }
            }
        }
        None => default_roots(),
    };

    let mut result = IndexResult::default();
    let mut files = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        collect_supported_files(&root, &mut files, &mut result);
    }
    files.sort();
    files.dedup();
    for file in files {
        if let Some(since_mtime_nanos) = options.since_mtime_nanos {
            let metadata = match fs::metadata(&file) {
                Ok(metadata) => metadata,
                Err(error) => {
                    record_file_error(&mut result, &file, &error.into());
                    continue;
                }
            };
            if metadata_mtime_nanos(&metadata).is_some_and(|mtime| mtime < since_mtime_nanos) {
                result.skipped_files += 1;
                continue;
            }
        }
        match index_file_with_options(
            pool,
            &file,
            detect_source_kind(&file).as_str(),
            IndexFileOptions {
                force: options.force,
            },
            storage,
        ) {
            Ok(file_result) => merge_result(&mut result, &file_result),
            Err(error) => {
                classify_path_error(&error, &mut result);
                tracing::warn!(path = %file.display(), error = %error, "Transcript file indexing failed");
                record_file_error(&mut result, &file, &error);
            }
        }
    }
    Ok(result)
}

pub fn index_file(pool: &DbPool, path: &Path, source_kind: &str) -> Result<IndexResult> {
    index_file_with_storage(pool, path, source_kind, None)
}

pub fn index_file_with_storage(
    pool: &DbPool,
    path: &Path,
    source_kind: &str,
    storage: Option<&StorageConfig>,
) -> Result<IndexResult> {
    index_file_with_options(
        pool,
        path,
        source_kind,
        IndexFileOptions::default(),
        storage,
    )
}

pub fn index_file_with_options(
    pool: &DbPool,
    path: &Path,
    source_kind: &str,
    options: IndexFileOptions,
    storage: Option<&StorageConfig>,
) -> Result<IndexResult> {
    validate_path(path)?;
    if !path.is_file() {
        return Err(PathScanError::ExpectedFile(path.to_path_buf()).into());
    }
    if let Some(storage) = storage {
        let outcome = enforce_storage_budget(pool, storage)?;
        if outcome.write_blocked {
            return Ok(IndexResult {
                discovered_files: 1,
                storage_blocked_chunks: 1,
                ..Default::default()
            });
        }
    }

    let canonical_path = path.canonicalize()?;
    let canonical = canonical_path.to_string_lossy().to_string();
    let mut source_kind = SourceKind::from_str(source_kind, &canonical_path);
    if source_kind == SourceKind::ExplicitFile {
        source_kind = detect_explicit_file_source_kind(&canonical_path)?;
    }
    let tool = source_kind.tool_name();
    let host = local_hostname();
    let mut fallback_project = project_for_file(source_kind, &canonical_path);
    let mut fallback_session_id = if source_kind == SourceKind::CodexSession {
        canonical_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(ToString::to_string)
    } else {
        None
    };
    let checkpoint_store = checkpoint::CheckpointStore::new(pool);
    let source_id = checkpoint_store.ensure_source(&canonical, source_kind.as_str())?;
    if options.force {
        checkpoint_store.reset_source(source_id, &canonical)?;
    }
    let current_metadata = FileMetadata::from_path_metadata(&canonical_path)?;
    let stored_metadata = if !options.force {
        checkpoint_store.source_metadata(source_id)?
    } else {
        None
    };
    if !options.force
        && source_kind != SourceKind::ExplicitFile
        && checkpoint_store.source_matches_metadata(
            source_id,
            current_metadata.size,
            current_metadata.mtime,
        )?
        && source_hash_matches(&canonical_path, stored_metadata.as_ref())?
    {
        return Ok(IndexResult {
            discovered_files: 1,
            ..Default::default()
        });
    }
    if source_kind == SourceKind::GeminiSession {
        return index_gemini_file(
            pool,
            storage,
            source_id,
            &canonical_path,
            &canonical,
            &current_metadata,
        );
    }
    let append_start = if !options.force {
        match stored_metadata.as_ref() {
            Some(metadata) => append_start_offset(metadata, &current_metadata)?,
            None => None,
        }
    } else {
        None
    };
    let mut imports = Vec::new();
    let mut batch = Vec::new();
    let mut skill_sources = Vec::new();
    let mut mcp_sources = Vec::new();
    let mut chunk_bytes = 0usize;
    let mut project_normalizer = ProjectNormalizer::default();
    let file = fs::File::open(&canonical_path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut line_no = if let Some(offset) = append_start {
        let counted = hash_prefix_and_count_lines(reader.get_mut(), &mut hasher, offset)?;
        let prefix_hash = hex_digest(&hasher.clone().finalize());
        if stored_metadata
            .as_ref()
            .and_then(|metadata| metadata.content_hash.as_ref())
            .is_some_and(|content_hash| prefix_hash != *content_hash)
        {
            hasher = Sha256::new();
            reader.get_mut().seek(SeekFrom::Start(0))?;
            0
        } else {
            reader.get_mut().seek(SeekFrom::Start(offset))?;
            counted
        }
    } else {
        0
    };
    let mut result = IndexResult {
        discovered_files: 1,
        ..Default::default()
    };

    loop {
        let Some(read_line) = read_bounded_line(&mut reader, Some(&mut hasher))? else {
            break;
        };
        if read_line.oversized {
            result.parse_errors += 1;
            let error = "transcript record exceeds max size";
            checkpoint_store.record_parse_error(source_id, line_no as i64, error, None)?;
            checkpoint_store.mark_error(source_id, error)?;
            line_no += 1;
            continue;
        }
        let line_text = read_line.text.trim_end_matches(['\r', '\n']);
        if line_text.trim().is_empty() {
            line_no += 1;
            continue;
        }
        update_codex_fallbacks(
            source_kind,
            line_text,
            &mut fallback_project,
            &mut fallback_session_id,
        );
        match parse_line_for_source(source_kind, line_text, &canonical_path, line_no) {
            Ok(Some(parsed)) => {
                let record_key = parsed.record_key;
                let skill_source = match source_kind {
                    SourceKind::CodexSession => {
                        if parsed.message.contains("<skill>") {
                            ChunkSkillSource::Codex(parsed.message.clone())
                        } else {
                            ChunkSkillSource::None
                        }
                    }
                    SourceKind::ClaudeProject | SourceKind::ExplicitFile => {
                        // Carry the already-parsed Claude JSON forward when the
                        // line could hold EITHER a skill attribution OR a hook
                        // attachment — both extractors read from this same
                        // value in flush_chunk (no second parse). The cheap
                        // substring guard keeps the common no-skill/no-hook
                        // line as `None`.
                        match &parsed.raw_value {
                            Some(value)
                                if line_text.contains("attributionSkill")
                                    || line_text.contains("hook_") =>
                            {
                                ChunkSkillSource::Claude(value.clone())
                            }
                            _ => ChunkSkillSource::None,
                        }
                    }
                    SourceKind::GeminiSession => ChunkSkillSource::None,
                };
                // Perf fix: gate the clone behind a cheap substring
                // pre-check, mirroring the skill-source gate above.
                // `extract_claude_mcp_events`/`extract_codex_mcp_events`
                // only ever match item/payload `type` fields containing
                // "tool_use"/"tool_result" (Claude) or "function_call"
                // (Codex, which also covers "function_call_output" as a
                // substring) — any line lacking these literals can never
                // produce an MCP event, so skipping the clone for them is
                // safe and avoids doubling per-record memory retention for
                // the (common) case of a tool-call-heavy transcript.
                let mcp_source = match source_kind {
                    SourceKind::CodexSession => match &parsed.raw_value {
                        Some(value) if line_text.contains("function_call") => {
                            ChunkMcpSource::Codex(value.clone())
                        }
                        _ => ChunkMcpSource::None,
                    },
                    SourceKind::ClaudeProject | SourceKind::ExplicitFile => match &parsed.raw_value
                    {
                        Some(value)
                            if line_text.contains("tool_use")
                                || line_text.contains("tool_result") =>
                        {
                            ChunkMcpSource::Claude(value.clone())
                        }
                        _ => ChunkMcpSource::None,
                    },
                    SourceKind::GeminiSession => ChunkMcpSource::None,
                };
                let message = scrub_ai_message(&parsed.message, None);
                let project_candidate = parsed
                    .ai_project
                    .as_deref()
                    .or(fallback_project.as_deref())
                    .map(|project| project_normalizer.normalize(project));
                let project = accept_metadata_field(
                    project_candidate.as_deref(),
                    MAX_AI_PROJECT_CHARS,
                    "ai_project",
                    &canonical,
                    &mut result,
                );
                let session_id = accept_metadata_field(
                    parsed
                        .session_id
                        .as_deref()
                        .or(fallback_session_id.as_deref()),
                    MAX_AI_SESSION_ID_CHARS,
                    "ai_session_id",
                    &canonical,
                    &mut result,
                );
                let transcript_path = accept_metadata_field(
                    Some(&canonical),
                    MAX_TRANSCRIPT_PATH_CHARS,
                    "ai_transcript_path",
                    &canonical,
                    &mut result,
                );
                let metadata_json = bounded_metadata_json(serde_json::json!({
                    "source_type": "transcript",
                    "source_kind": source_kind.as_str(),
                    "tool": tool,
                    "canonical_path": canonical,
                    "line_no": line_no,
                    "record_key": record_key,
                    "event_kind": parsed.event_kind,
                    "content_scrubbed": true,
                }));
                let entry = LogBatchEntry {
                    timestamp: normalize_timestamp(parsed.timestamp.as_deref())?,
                    hostname: host.clone(),
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
                    ai_session_id: session_id,
                    ai_transcript_path: transcript_path,
                    metadata_json: Some(metadata_json),
                    http_status: None,
                    auth_outcome: None,
                    dns_blocked: None,
                    event_action: None,
                    parse_error: None,
                };
                chunk_bytes = chunk_bytes.saturating_add(log_entry_string_bytes(&entry));
                batch.push(entry);
                imports.push(record_key);
                skill_sources.push(skill_source);
                mcp_sources.push(mcp_source);
                if batch.len() >= MAX_INDEX_CHUNK_RECORDS || chunk_bytes >= MAX_INDEX_CHUNK_BYTES {
                    if !flush_chunk(
                        pool,
                        storage,
                        source_id,
                        &mut batch,
                        &mut imports,
                        &mut skill_sources,
                        &mut mcp_sources,
                        None,
                        &mut result,
                    )? {
                        return Ok(result);
                    }
                    chunk_bytes = 0;
                }
            }
            Ok(None) => {}
            Err(error) => {
                result.parse_errors += 1;
                checkpoint_store.record_parse_error(
                    source_id,
                    line_no as i64,
                    &error.to_string(),
                    Some(&record_preview(line_text)),
                )?;
                checkpoint_store.mark_error(source_id, &error.to_string())?;
            }
        }
        line_no += 1;
    }

    let final_metadata = FileMetadata::from_path_metadata(&canonical_path)?;
    let unchanged_during_scan = current_metadata.same_size_and_mtime(&final_metadata);
    let file_metadata = current_metadata.with_hash(&hasher.finalize());
    let completion_metadata = unchanged_during_scan
        .then_some(file_metadata)
        .filter(|_| result.parse_errors == 0);
    if result.parse_errors > 0 {
        flush_chunk(
            pool,
            storage,
            source_id,
            &mut batch,
            &mut imports,
            &mut skill_sources,
            &mut mcp_sources,
            None,
            &mut result,
        )?;
        checkpoint_store.mark_error(
            source_id,
            &format!(
                "{} transcript record(s) failed to parse",
                result.parse_errors
            ),
        )?;
        return Ok(result);
    }
    let _ = flush_chunk(
        pool,
        storage,
        source_id,
        &mut batch,
        &mut imports,
        &mut skill_sources,
        &mut mcp_sources,
        completion_metadata.as_ref(),
        &mut result,
    )?;
    Ok(result)
}

fn append_start_offset(
    stored: &checkpoint::SourceMetadata,
    current: &FileMetadata,
) -> Result<Option<u64>> {
    let Some(stored_size) = stored.file_size else {
        return Ok(None);
    };
    if stored.last_error.is_some() || stored_size <= 0 {
        return Ok(None);
    }
    if stored.content_hash.is_none() {
        return Ok(None);
    }
    let Ok(stored_size) = u64::try_from(stored_size) else {
        return Ok(None);
    };
    let last_offset = stored
        .last_offset
        .and_then(|offset| u64::try_from(offset).ok())
        .unwrap_or(stored_size);
    if stored_size >= current.size || last_offset > current.size {
        return Ok(None);
    }
    if last_offset == 0 || last_offset != stored_size {
        return Ok(None);
    }
    Ok(Some(last_offset))
}

fn source_hash_matches(path: &Path, stored: Option<&checkpoint::SourceMetadata>) -> Result<bool> {
    let Some(content_hash) = stored.and_then(|metadata| metadata.content_hash.as_ref()) else {
        return Ok(false);
    };
    Ok(hash_file(path)? == *content_hash)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 8192];
    let mut hasher = Sha256::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn hash_prefix_and_count_lines(
    file: &mut fs::File,
    hasher: &mut Sha256,
    offset: u64,
) -> Result<usize> {
    file.seek(SeekFrom::Start(0))?;
    let mut remaining = offset;
    let mut buffer = [0u8; 8192];
    let mut lines = 0usize;
    while remaining > 0 {
        let to_read = buffer.len().min(remaining as usize);
        let read = file.read(&mut buffer[..to_read])?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        lines += buffer[..read].iter().filter(|byte| **byte == b'\n').count();
        remaining -= read as u64;
    }
    Ok(lines)
}

pub fn list_checkpoints(
    pool: &DbPool,
    options: &CheckpointListOptions,
) -> Result<Vec<CheckpointEntry>> {
    checkpoint::CheckpointStore::new(pool).list_checkpoints(options)
}

pub fn list_parse_errors(
    pool: &DbPool,
    options: &ParseErrorListOptions,
) -> Result<Vec<ParseErrorEntry>> {
    checkpoint::CheckpointStore::new(pool).list_parse_errors(options)
}

pub fn prune_checkpoints(
    pool: &DbPool,
    options: &PruneCheckpointsOptions,
) -> Result<PruneCheckpointsResult> {
    checkpoint::CheckpointStore::new(pool).prune_checkpoints(options)
}

pub fn ai_doctor(pool: &DbPool, db_path: &Path) -> Result<AiDoctorReport> {
    checkpoint::CheckpointStore::new(pool).doctor(db_path)
}

pub fn ai_indexing_health(
    pool: &DbPool,
    process_start_time: Option<&str>,
) -> Result<AiIndexingHealth> {
    checkpoint::CheckpointStore::new(pool).indexing_health(process_start_time)
}

#[allow(clippy::too_many_arguments)]
fn flush_chunk(
    pool: &DbPool,
    storage: Option<&StorageConfig>,
    source_id: i64,
    batch: &mut Vec<LogBatchEntry>,
    imports: &mut Vec<String>,
    skill_sources: &mut Vec<ChunkSkillSource>,
    mcp_sources: &mut Vec<ChunkMcpSource>,
    completion_metadata: Option<&FileMetadata>,
    result: &mut IndexResult,
) -> Result<bool> {
    if batch.is_empty() {
        skill_sources.clear();
        mcp_sources.clear();
        if let Some(file_metadata) = completion_metadata {
            let mut conn = crate::db::write_conn(pool)?;
            let tx = conn.transaction()?;
            checkpoint::update_source_metadata_in_tx(&tx, source_id, file_metadata)?;
            tx.commit()?;
            result.checkpoint_updates += 1;
        }
        return Ok(true);
    }

    if let Some(storage) = storage {
        let outcome = enforce_storage_budget(pool, storage)?;
        if outcome.write_blocked {
            result.storage_blocked_chunks += 1;
            batch.clear();
            imports.clear();
            skill_sources.clear();
            mcp_sources.clear();
            return Ok(false);
        }
    }

    let mut conn = crate::db::write_conn(pool)?;
    let tx = conn.transaction()?;
    let claimed = checkpoint::claim_imports_in_tx(&tx, source_id, imports)?;
    let mut claimed_batch = Vec::with_capacity(batch.len());
    let mut claimed_skill_sources = Vec::with_capacity(skill_sources.len());
    let mut claimed_mcp_sources = Vec::with_capacity(mcp_sources.len());
    let mut skipped_dupes = 0usize;
    for (((entry, claimed), skill_source), mcp_source) in batch
        .drain(..)
        .zip(claimed)
        .zip(skill_sources.drain(..))
        .zip(mcp_sources.drain(..))
    {
        if claimed {
            claimed_batch.push(entry);
            claimed_skill_sources.push(skill_source);
            claimed_mcp_sources.push(mcp_source);
        } else {
            skipped_dupes += 1;
        }
    }
    if !claimed_batch.is_empty() {
        let log_ids = insert_logs_batch_in_tx(&tx, &claimed_batch)?;
        let mut skill_inserts = Vec::new();
        let mut mcp_inserts = Vec::new();
        let mut hook_inserts = Vec::new();
        for (((entry, log_id), skill_source), mcp_source) in claimed_batch
            .iter()
            .zip(log_ids.iter().copied())
            .zip(claimed_skill_sources.iter())
            .zip(claimed_mcp_sources.iter())
        {
            let extracted = match skill_source {
                ChunkSkillSource::Claude(value) => extract_claude_skill_events(value),
                ChunkSkillSource::Codex(text) => extract_codex_skill_events(text),
                ChunkSkillSource::None => Vec::new(),
            };
            for event in extracted {
                skill_inserts.push(SkillEventInsert {
                    log_id,
                    ai_tool: entry.ai_tool.clone().unwrap_or_default(),
                    ai_project: entry.ai_project.clone(),
                    ai_session_id: entry.ai_session_id.clone(),
                    hostname: entry.hostname.clone(),
                    timestamp: entry.timestamp.clone(),
                    event,
                });
            }
            let extracted_mcp = match mcp_source {
                ChunkMcpSource::Claude(value) => extract_claude_mcp_events(value),
                ChunkMcpSource::Codex(value) => extract_codex_mcp_events(value),
                ChunkMcpSource::None => Vec::new(),
            };
            for event in extracted_mcp {
                mcp_inserts.push(McpEventInsert {
                    log_id,
                    ai_tool: entry.ai_tool.clone().unwrap_or_default(),
                    ai_project: entry.ai_project.clone(),
                    ai_session_id: entry.ai_session_id.clone(),
                    hostname: entry.hostname.clone(),
                    timestamp: entry.timestamp.clone(),
                    event,
                });
            }

            // Hook runtime events reuse the already-parsed Claude `value` from
            // the same side-channel (no second JSON parse). Only Claude
            // transcripts carry a runtime hook attachment shape; Codex/Gemini
            // rows produce none (config/trust-state hook evidence is collected
            // separately by `crate::hook_config`, not at ingest time).
            let hook_events = match skill_source {
                ChunkSkillSource::Claude(value) => extract_claude_hook_events(value),
                _ => Vec::new(),
            };
            for event in hook_events {
                hook_inserts.push(HookEventInsert {
                    log_id: Some(log_id),
                    ai_tool: entry.ai_tool.clone().unwrap_or_default(),
                    ai_project: entry.ai_project.clone(),
                    ai_session_id: entry.ai_session_id.clone(),
                    hostname: entry.hostname.clone(),
                    timestamp: entry.timestamp.clone(),
                    event,
                });
            }
        }
        if !skill_inserts.is_empty() {
            insert_skill_events_in_tx(&tx, &skill_inserts)?;
        }
        if !mcp_inserts.is_empty() {
            insert_mcp_events_in_tx(&tx, &mcp_inserts)?;
        }
        if !hook_inserts.is_empty() {
            insert_hook_events_in_tx(&tx, &hook_inserts)?;
        }
    }
    if let Some(file_metadata) = completion_metadata {
        checkpoint::update_source_metadata_in_tx(&tx, source_id, file_metadata)?;
    }
    tx.commit()?;
    if !claimed_batch.is_empty() {
        crate::db::agent_observatory::notify_projection_work();
    }
    result.ingested += claimed_batch.len();
    result.skipped_dupes += skipped_dupes;
    if completion_metadata.is_some() {
        result.checkpoint_updates += 1;
    }
    imports.clear();
    Ok(true)
}

fn collect_supported_files(path: &Path, files: &mut Vec<PathBuf>, result: &mut IndexResult) {
    if let Err(error) = validate_path(path) {
        classify_path_error(&error, result);
        record_file_error(result, path, &error);
        return;
    }
    if path.is_file() {
        if supported_discovered_file(path) {
            files.push(path.to_path_buf());
        } else {
            result.unsupported_files += 1;
        }
        return;
    }
    if !should_descend_transcript_dir(path) {
        return;
    }

    let mut entries = Vec::new();
    let read_dir = match fs::read_dir(path) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            record_discovered_path_error(result, path, &error.into());
            return;
        }
    };
    for entry in read_dir {
        match entry.with_context(|| format!("failed to read entry under {}", path.display())) {
            Ok(entry) => entries.push(entry.path()),
            Err(error) => {
                record_discovered_path_error(result, path, &error);
            }
        }
    }
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            collect_supported_files(&entry, files, result);
        } else if supported_discovered_file(&entry) {
            files.push(entry);
        } else {
            result.unsupported_files += 1;
        }
    }
}

fn supported_discovered_file(path: &Path) -> bool {
    matches!(path.extension().and_then(|ext| ext.to_str()), Some("jsonl"))
        || gemini::is_chat_file(path)
}

fn detect_explicit_file_source_kind(path: &Path) -> Result<SourceKind> {
    if gemini::is_chat_file(path) {
        return Ok(SourceKind::GeminiSession);
    }
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line_no = 0usize;
    while line_no < 50 {
        // Sniffing source kind only needs the text, not the checkpoint digest.
        let Some(read_line) = read_bounded_line(&mut reader, None)? else {
            return Ok(SourceKind::ExplicitFile);
        };
        if read_line.oversized {
            return Ok(SourceKind::ExplicitFile);
        }
        let line = read_line.text.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            line_no += 1;
            continue;
        }
        if looks_like_codex_record(line) {
            return Ok(SourceKind::CodexSession);
        }
        match claude::parse_line(line, path, line_no) {
            Ok(Some(_)) => return Ok(SourceKind::ExplicitFile),
            Ok(None) => {}
            Err(_) => return Ok(SourceKind::ExplicitFile),
        }
        line_no += 1;
    }
    Ok(SourceKind::ExplicitFile)
}

fn looks_like_codex_record(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("session_meta" | "response_item" | "event_msg" | "turn_context")
    ) || value
        .get("payload")
        .and_then(|payload| payload.get("type"))
        .and_then(serde_json::Value::as_str)
        .is_some()
}

pub(crate) fn detect_source_kind(path: &Path) -> SourceKind {
    let display = path.to_string_lossy();
    if display.contains(".codex/sessions") || display.contains(".codex/worktrees") {
        SourceKind::CodexSession
    } else if display.contains(".gemini/tmp") {
        SourceKind::GeminiSession
    } else if display.contains(".claude/projects") {
        SourceKind::ClaudeProject
    } else {
        SourceKind::ExplicitFile
    }
}

impl SourceKind {
    fn from_str(source_kind: &str, path: &Path) -> Self {
        match source_kind {
            "codex_session" => Self::CodexSession,
            "claude_project" => Self::ClaudeProject,
            "gemini_session" => Self::GeminiSession,
            _ => detect_source_kind(path),
        }
    }

    pub(crate) fn tool_name(self) -> &'static str {
        match self {
            Self::CodexSession => "codex",
            Self::GeminiSession => "gemini",
            Self::ClaudeProject | Self::ExplicitFile => "claude",
        }
    }
}

pub(crate) fn parse_line_for_source(
    source_kind: SourceKind,
    line: &str,
    path: &Path,
    line_no: usize,
) -> Result<Option<ParsedTranscriptRecord>> {
    match source_kind {
        SourceKind::CodexSession => codex::parse_line(line, path, line_no),
        // Gemini sessions are whole-file JSON and are diverted to
        // `index_gemini_file` before this per-line loop is ever reached, so this
        // arm is structurally unreachable. Keep the invariant explicit rather
        // than carrying a dead line-parser that looks load-bearing.
        SourceKind::GeminiSession => {
            unreachable!("gemini sessions are indexed whole-file by index_gemini_file")
        }
        SourceKind::ClaudeProject | SourceKind::ExplicitFile => {
            claude::parse_line(line, path, line_no)
        }
    }
}

pub(crate) fn project_for_file(source_kind: SourceKind, path: &Path) -> Option<String> {
    match source_kind {
        SourceKind::ClaudeProject => project_from_transcript_path(&path.to_string_lossy()),
        SourceKind::CodexSession => None,
        SourceKind::GeminiSession => None,
        SourceKind::ExplicitFile => std::env::current_dir()
            .ok()
            .map(|path| normalize_local_ai_project_path(&path.to_string_lossy())),
    }
}

pub(crate) fn update_codex_fallbacks(
    source_kind: SourceKind,
    line: &str,
    fallback_project: &mut Option<String>,
    fallback_session_id: &mut Option<String>,
) {
    if source_kind != SourceKind::CodexSession {
        return;
    }
    if let Some(project) = codex::project_from_line(line) {
        *fallback_project = Some(project);
    }
    if let Some(session_id) = codex::session_id_from_line(line) {
        *fallback_session_id = Some(session_id);
    }
}

#[derive(Default)]
struct ProjectNormalizer {
    cache: HashMap<String, String>,
}

impl ProjectNormalizer {
    fn normalize(&mut self, project: &str) -> String {
        if let Some(normalized) = self.cache.get(project) {
            return normalized.clone();
        }
        let normalized = normalize_local_ai_project_path(project);
        self.cache.insert(project.to_string(), normalized.clone());
        normalized
    }
}

fn index_gemini_file(
    pool: &DbPool,
    storage: Option<&StorageConfig>,
    source_id: i64,
    path: &Path,
    canonical: &str,
    current_metadata: &FileMetadata,
) -> Result<IndexResult> {
    let checkpoint_store = checkpoint::CheckpointStore::new(pool);
    let raw = fs::read_to_string(path).context("Gemini transcript is not valid UTF-8")?;
    let file_hash = hash_text(&raw);
    let parsed = gemini::parse_file(&raw, path)?;
    let mut result = IndexResult {
        discovered_files: 1,
        ..Default::default()
    };

    // A chat file with no `messages` array is almost certainly an upstream
    // schema change. Surface it as a recorded parse error and do NOT write
    // completion metadata, so the file is re-examined next scan instead of being
    // silently checkpointed as fully indexed (which would hide every message).
    if parsed.missing_messages {
        let error = "Gemini chat file has no 'messages' array — upstream schema may have changed";
        result.parse_errors += 1;
        checkpoint_store.record_parse_error(source_id, 0, error, None)?;
        checkpoint_store.mark_error(source_id, error)?;
        tracing::warn!(path = %path.display(), "{error}");
        return Ok(result);
    }
    if parsed.skipped_empty > 0 {
        tracing::debug!(
            path = %path.display(),
            skipped = parsed.skipped_empty,
            "gemini: skipped messages with no extractable text content"
        );
    }

    let mut batch = Vec::new();
    let mut imports = Vec::new();
    // Gemini rows never produce skill or MCP events — Gemini extraction is
    // explicitly out of scope for this phase. These vectors stay empty
    // (one ChunkSkillSource::None / ChunkMcpSource::None pushed per record)
    // purely so flush_chunk's shared signature is satisfied uniformly
    // across all source kinds.
    let mut skill_sources: Vec<ChunkSkillSource> = Vec::new();
    let mut mcp_sources: Vec<ChunkMcpSource> = Vec::new();
    let mut chunk_bytes = 0usize;
    let host = local_hostname();
    let tool = SourceKind::GeminiSession.tool_name();
    let mut project_normalizer = ProjectNormalizer::default();
    for (record_index, record) in parsed.records.into_iter().enumerate() {
        // A single malformed timestamp must not abort the whole file. Treat it
        // like the JSONL per-line path: record the error, skip the record, keep
        // ingesting the rest, and refuse the completion checkpoint at the end.
        let timestamp = match normalize_timestamp(record.timestamp.as_deref()) {
            Ok(timestamp) => timestamp,
            Err(error) => {
                result.parse_errors += 1;
                checkpoint_store.record_parse_error(
                    source_id,
                    record_index as i64,
                    &error.to_string(),
                    Some(&record_preview(&record.message)),
                )?;
                checkpoint_store.mark_error(source_id, &error.to_string())?;
                continue;
            }
        };
        let record_key = record.record_key;
        let message = scrub_ai_message(&record.message, None);
        let project_candidate = record
            .ai_project
            .as_deref()
            .map(|project| project_normalizer.normalize(project));
        let project = accept_metadata_field(
            project_candidate.as_deref(),
            MAX_AI_PROJECT_CHARS,
            "ai_project",
            canonical,
            &mut result,
        );
        let session_id = accept_metadata_field(
            record.session_id.as_deref(),
            MAX_AI_SESSION_ID_CHARS,
            "ai_session_id",
            canonical,
            &mut result,
        );
        let transcript_path = accept_metadata_field(
            Some(canonical),
            MAX_TRANSCRIPT_PATH_CHARS,
            "ai_transcript_path",
            canonical,
            &mut result,
        );
        let metadata_json = bounded_metadata_json(serde_json::json!({
            "source_type": "transcript",
            "source_kind": SourceKind::GeminiSession.as_str(),
            "tool": tool,
            "canonical_path": canonical,
            "record_index": record_index,
            "record_key": record_key,
            "event_kind": record.event_kind,
            "content_scrubbed": true,
        }));
        let entry = LogBatchEntry {
            timestamp,
            hostname: host.clone(),
            facility: Some("transcript".to_string()),
            severity: "info".to_string(),
            app_name: Some(format!("{tool}-transcript")),
            process_id: None,
            raw: message.clone(),
            message,
            source_ip: format!("transcript://{}", SourceKind::GeminiSession.as_str()),
            docker_checkpoint: None,
            ai_tool: Some(tool.to_string()),
            ai_project: project,
            ai_session_id: session_id,
            ai_transcript_path: transcript_path,
            metadata_json: Some(metadata_json),
            http_status: None,
            auth_outcome: None,
            dns_blocked: None,
            event_action: None,
            parse_error: None,
        };
        chunk_bytes = chunk_bytes.saturating_add(log_entry_string_bytes(&entry));
        batch.push(entry);
        imports.push(record_key);
        skill_sources.push(ChunkSkillSource::None);
        mcp_sources.push(ChunkMcpSource::None);
        if batch.len() >= MAX_INDEX_CHUNK_RECORDS || chunk_bytes >= MAX_INDEX_CHUNK_BYTES {
            if !flush_chunk(
                pool,
                storage,
                source_id,
                &mut batch,
                &mut imports,
                &mut skill_sources,
                &mut mcp_sources,
                None,
                &mut result,
            )? {
                return Ok(result);
            }
            chunk_bytes = 0;
        }
    }
    let final_metadata = FileMetadata::from_path_metadata(path)?;
    let completion_metadata = current_metadata
        .same_size_and_mtime(&final_metadata)
        .then(|| current_metadata.clone().with_hash_from_hex(file_hash))
        .filter(|_| result.parse_errors == 0);
    if result.parse_errors > 0 {
        flush_chunk(
            pool,
            storage,
            source_id,
            &mut batch,
            &mut imports,
            &mut skill_sources,
            &mut mcp_sources,
            None,
            &mut result,
        )?;
        checkpoint_store.mark_error(
            source_id,
            &format!(
                "{} transcript record(s) failed to parse",
                result.parse_errors
            ),
        )?;
        return Ok(result);
    }
    let _ = flush_chunk(
        pool,
        storage,
        source_id,
        &mut batch,
        &mut imports,
        &mut skill_sources,
        &mut mcp_sources,
        completion_metadata.as_ref(),
        &mut result,
    )?;
    Ok(result)
}

fn merge_result(total: &mut IndexResult, next: &IndexResult) {
    total.discovered_files += next.discovered_files;
    total.ingested += next.ingested;
    total.skipped_dupes += next.skipped_dupes;
    total.parse_errors += next.parse_errors;
    total.skipped_files += next.skipped_files;
    total.unsupported_files += next.unsupported_files;
    total.skipped_symlinks += next.skipped_symlinks;
    total.skipped_unsafe_paths += next.skipped_unsafe_paths;
    total.storage_blocked_chunks += next.storage_blocked_chunks;
    total.dropped_metadata_fields += next.dropped_metadata_fields;
    total.checkpoint_updates += next.checkpoint_updates;
    total.file_errors.extend(next.file_errors.iter().cloned());
}

fn record_preview(value: &str) -> String {
    scrub_ai_message(&truncate_chars(value, 240), None)
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value.chars().take(max).collect()
    }
}

fn record_file_error(result: &mut IndexResult, path: &Path, error: &anyhow::Error) {
    result.skipped_files += 1;
    result.file_errors.push(IndexFileError {
        path: path.display().to_string(),
        error: error.to_string(),
    });
}

fn record_discovered_path_error(result: &mut IndexResult, path: &Path, error: &anyhow::Error) {
    result.skipped_files += 1;
    if is_permission_denied(error) {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "Skipping unreadable transcript path during discovery"
        );
    }
    result.file_errors.push(IndexFileError {
        path: path.display().to_string(),
        error: error.to_string(),
    });
}

fn is_permission_denied(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
}

fn accept_metadata_field(
    value: Option<&str>,
    max_chars: usize,
    field: &'static str,
    file_key: &str,
    result: &mut IndexResult,
) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() > max_chars {
        if result
            .dropped_metadata_field_keys
            .insert(format!("{file_key}:{field}"))
        {
            result.dropped_metadata_fields += 1;
        }
        tracing::warn!(
            field,
            max_chars,
            actual_chars = trimmed.chars().count(),
            "Dropping oversized AI transcript metadata field"
        );
        return None;
    }
    Some(trimmed.to_string())
}

fn normalize_timestamp(timestamp: Option<&str>) -> Result<String> {
    match timestamp {
        Some(value) => Ok(chrono::DateTime::parse_from_rfc3339(value)
            .with_context(|| format!("invalid transcript timestamp: {value}"))?
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string()),
        None => Ok(chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string()),
    }
}

/// Warn once per process when the hostname cannot be resolved. Rows filed under
/// the literal `localhost` do not correspond to any fleet host, so correlation
/// queries for the real host silently return nothing — worth one loud line.
fn warn_unresolved_hostname() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "could not resolve a hostname (gethostname failed and $HOSTNAME is unset); \
             falling back to \"localhost\" — forwarded rows will be misattributed"
        );
    });
}

pub(crate) fn local_hostname() -> String {
    #[cfg(unix)]
    {
        let mut buf = vec![0u8; 256];
        let result = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if result == 0 {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            if let Ok(name) = std::str::from_utf8(&buf[..len]) {
                let name = name.trim();
                if !name.is_empty() && name != "localhost" {
                    return name.to_string();
                }
            }
        }
        crate::env::var("HOSTNAME").unwrap_or_else(|_| {
            warn_unresolved_hostname();
            "localhost".to_string()
        })
    }
    #[cfg(not(unix))]
    {
        // On Windows use COMPUTERNAME; fall back to HOSTNAME then "localhost".
        for var in &["COMPUTERNAME", "HOSTNAME"] {
            if let Ok(name) = crate::env::var(var) {
                let name = name.trim().to_string();
                if !name.is_empty() && name != "localhost" {
                    return name;
                }
            }
        }
        "localhost".to_string()
    }
}

fn log_entry_string_bytes(entry: &LogBatchEntry) -> usize {
    entry
        .timestamp
        .len()
        .saturating_add(entry.hostname.len())
        .saturating_add(entry.facility.as_ref().map_or(0, String::len))
        .saturating_add(entry.severity.len())
        .saturating_add(entry.app_name.as_ref().map_or(0, String::len))
        .saturating_add(entry.process_id.as_ref().map_or(0, String::len))
        .saturating_add(entry.raw.len())
        .saturating_add(entry.message.len())
        .saturating_add(entry.source_ip.len())
        .saturating_add(entry.docker_checkpoint.as_ref().map_or(0, |checkpoint| {
            checkpoint
                .host_name
                .len()
                .saturating_add(checkpoint.container_id.len())
                .saturating_add(checkpoint.timestamp.len())
        }))
        .saturating_add(entry.ai_tool.as_ref().map_or(0, String::len))
        .saturating_add(entry.ai_project.as_ref().map_or(0, String::len))
        .saturating_add(entry.ai_session_id.as_ref().map_or(0, String::len))
        .saturating_add(entry.ai_transcript_path.as_ref().map_or(0, String::len))
        .saturating_add(entry.metadata_json.as_ref().map_or(0, String::len))
}

struct ReadLine {
    text: String,
    oversized: bool,
}

/// Read one newline-delimited record, capping it at `MAX_RECORD_SIZE_BYTES`.
///
/// `hasher` is optional: the ingest/checkpoint path passes `Some(..)` to fold
/// each byte into the source-file digest, while callers that only need the
/// text (e.g. `read_transcript_lines`, the skill-event backfill's historical
/// recovery) pass `None` to skip the SHA-256 work entirely.
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    mut hasher: Option<&mut Sha256>,
) -> Result<Option<ReadLine>> {
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() && !oversized {
                return Ok(None);
            }
            break;
        }
        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let take_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        if let Some(hasher) = hasher.as_deref_mut() {
            hasher.update(&available[..take_len]);
        }
        if !oversized {
            // Copy one sentinel byte past the inclusive record limit so we can
            // detect oversized records while still accepting exactly-limit rows.
            let remaining = MAX_RECORD_SIZE_BYTES
                .saturating_add(1)
                .saturating_sub(line.len());
            let copy_len = take_len.min(remaining);
            line.extend_from_slice(&available[..copy_len]);
            if line.len() > MAX_RECORD_SIZE_BYTES {
                oversized = true;
                line.clear();
            }
        }
        reader.consume(take_len);
        if newline_pos.is_some() {
            break;
        }
    }
    if oversized {
        return Ok(Some(ReadLine {
            text: String::new(),
            oversized: true,
        }));
    }
    let text = String::from_utf8(line).context("transcript record is not valid UTF-8")?;
    Ok(Some(ReadLine {
        text,
        oversized: false,
    }))
}

/// Recover specific 0-based lines from a transcript file, using the same
/// bounded, newline-delimited record semantics as the ingest path
/// (`read_bounded_line` / `MAX_RECORD_SIZE_BYTES`) so `line_no` values recorded
/// at ingest time resolve to the same physical lines here. Opens and scans the
/// file once, stopping as soon as every requested line has been found.
///
/// Returns only the requested lines that were located and are within the
/// record-size bound. A line beyond EOF (file truncated/rotated since ingest)
/// or one that now exceeds `MAX_RECORD_SIZE_BYTES` (file corrupted/rewritten)
/// is simply absent from the returned map — callers treat that as
/// "source unavailable" rather than an error, since this is best-effort
/// historical recovery, not the live ingest path. Trailing `\r`/`\n` is
/// trimmed to match how the ingest path normalizes each record before parsing.
pub(crate) fn read_transcript_lines(
    path: &Path,
    wanted: &HashSet<usize>,
) -> Result<HashMap<usize, String>> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut out = HashMap::new();
    let mut line_no = 0usize;
    let mut remaining = wanted.len();
    while remaining > 0 {
        let Some(read_line) = read_bounded_line(&mut reader, None)? else {
            break; // EOF before every requested line was found
        };
        if !read_line.oversized && wanted.contains(&line_no) {
            let text = read_line.text.trim_end_matches(['\r', '\n']).to_string();
            out.insert(line_no, text);
            remaining -= 1;
        }
        line_no += 1;
    }
    Ok(out)
}

fn default_roots() -> Vec<PathBuf> {
    crate::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| {
            vec![
                home.join(".claude/projects"),
                home.join(".codex/sessions"),
                home.join(".codex/worktrees"),
                home.join(".gemini/tmp"),
            ]
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "scanner_tests.rs"]
mod tests;

#[derive(Debug, Clone)]
pub(crate) struct ParsedTranscriptRecord {
    pub record_key: String,
    pub timestamp: Option<String>,
    pub message: String,
    pub session_id: Option<String>,
    pub ai_project: Option<String>,
    pub event_kind: String,
    /// The already-parsed raw JSON value for Claude transcript lines (`None`
    /// for Codex/Gemini, which don't need it — Codex's skill-tag scanner
    /// reads `message` directly; Gemini never produces skill events). Lets
    /// skill-event extraction (Task 6) reuse the JSON parse `parse_line`
    /// already did internally, instead of re-parsing `line_text` a second
    /// time (eng review Fix 1 — see Task 2).
    pub raw_value: Option<serde_json::Value>,
}

pub(crate) fn transcript_event_kind(value: &serde_json::Value) -> String {
    let payload = value.get("payload").unwrap_or(value);
    let kind = payload
        .get("type")
        .or_else(|| value.get("type"))
        .or_else(|| payload.get("role"))
        .or_else(|| value.get("role"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    match kind {
        "user" | "human" => "user",
        "assistant" => "assistant",
        "tool" | "tool_use" | "tool_result" | "function_call" | "function_call_output" => "tool",
        value if value.contains("hook") => "hook",
        "reasoning" => "reasoning",
        "error" => "error",
        "event_msg" | "turn_context" | "session_meta" | "status" => "status",
        _ => "unknown",
    }
    .to_string()
}

#[derive(Debug, Clone)]
pub(crate) struct FileMetadata {
    pub size: u64,
    pub mtime: Option<i64>,
    pub content_hash: String,
}

impl FileMetadata {
    fn from_path_metadata(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            size: metadata.len(),
            mtime: metadata_mtime_nanos(&metadata),
            content_hash: String::new(),
        })
    }

    fn with_hash(mut self, hash: &[u8]) -> Self {
        self.content_hash = hex_digest(hash);
        self
    }

    fn with_hash_from_hex(mut self, hash: String) -> Self {
        self.content_hash = hash;
        self
    }

    fn same_size_and_mtime(&self, other: &Self) -> bool {
        self.size == other.size && self.mtime == other.mtime
    }
}

fn metadata_mtime_nanos(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn record_key_from_line(
    value: &serde_json::Value,
    line: &str,
    line_no: usize,
) -> String {
    value
        .get("uuid")
        .or_else(|| value.get("id"))
        .or_else(|| value.pointer("/payload/id"))
        .and_then(serde_json::Value::as_str)
        .map(|id| format!("id:{id}"))
        .unwrap_or_else(|| format!("line:{line_no}:hash:{}", hash_text(line)))
}

pub(crate) fn hash_text(text: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}
