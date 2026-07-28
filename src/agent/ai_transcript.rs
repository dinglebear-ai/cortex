//! Forwards local AI transcript changes (Claude/Codex/Gemini) to the central
//! cortex server via `POST /v1/ai-transcripts`, mirroring the local-only
//! `cortex sessions watch` path but over the network — one more supervised
//! stream inside `cortex agent`, alongside docker/journald/file-tail.
//!
//! Claude/Codex are append-only JSONL — tailed by byte/line offset via
//! `read_new_lines`. Gemini sessions are a single whole-file JSON object
//! (new messages appended, not a growing log file), so they're handled
//! separately in `scan_and_forward`: re-parsed in full each cycle via
//! `scanner::gemini::parse_file`, with the checkpoint tracking a *record
//! index* instead of a byte offset.
//!
//! Unlike the local watcher (`ai_watch.rs`, notify-based, debounced), this
//! forwarder polls on a fixed interval and tracks a simple per-file
//! "already forwarded" checkpoint (lines for Claude/Codex, records for
//! Gemini) in a local JSON state file. Polling (rather than filesystem
//! notify) keeps the agent's dependency footprint small and matches the
//! reliability bar of the other agent streams, which all tolerate
//! multi-second latency already.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ai_project::normalize_local_ai_project_path;
use crate::ai_transcript_ingest::{AiTranscriptIngestRequest, AiTranscriptRecord};
use crate::scanner;

/// Cap on the *aggregate* batch per scan cycle, across every transcript file
/// combined — stays comfortably under the server's `MAX_RECORDS_PER_BATCH`
/// (2,000) and any fronting proxy's request-size limit. A backlog larger
/// than this drains over several poll cycles instead of one oversized POST.
const MAX_BATCH_RECORDS: usize = 500;
const CODEX_PREFIX_METADATA_SCAN_LINES: usize = 200;

#[derive(Debug, Clone)]
pub struct AiTranscriptForwardConfig {
    pub roots: Vec<PathBuf>,
    /// Central server base URL, e.g. `http://tootie:3100`.
    pub target: String,
    pub token: Option<String>,
    pub hostname: String,
    pub checkpoint_path: PathBuf,
    pub poll_interval: Duration,
}

impl AiTranscriptForwardConfig {
    pub fn new(target: String, token: Option<String>, checkpoint_path: PathBuf) -> Self {
        Self {
            roots: scanner::default_transcript_roots(),
            target,
            token,
            hostname: scanner::local_hostname(),
            checkpoint_path,
            poll_interval: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Checkpoint {
    /// Canonical path string -> lines already forwarded.
    files: HashMap<String, usize>,
    /// In-process malformed Gemini revisions already warned about. This is
    /// deliberately not persisted: one warning per bad content revision per
    /// agent process is enough to surface the problem without feeding the
    /// same warning back through journald every poll cycle.
    #[serde(skip)]
    gemini_parse_failures: HashMap<String, u64>,
}

fn load_checkpoint(path: &Path) -> Checkpoint {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create checkpoint dir {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec(checkpoint)?;
    fs::write(path, bytes)
        .with_context(|| format!("failed to write checkpoint file {}", path.display()))
}

fn gemini_content_fingerprint(raw: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    hasher.finish()
}

fn should_warn_gemini_parse_failure(checkpoint: &mut Checkpoint, key: &str, raw: &str) -> bool {
    let fingerprint = gemini_content_fingerprint(raw);
    checkpoint
        .gemini_parse_failures
        .insert(key.to_string(), fingerprint)
        != Some(fingerprint)
}

/// Recursively collect supported transcript files under `root` (mirrors
/// `scanner`'s discovery rules via the public `is_supported_transcript_file`
/// predicate, without pulling in the local-indexing `IndexResult` coupling
/// that `scanner::collect_supported_files` carries).
fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    if root.is_file() {
        if scanner::is_supported_transcript_file(root) {
            out.push(root.to_path_buf());
        }
        return;
    }
    if !scanner::should_descend_transcript_dir(root) {
        return;
    }
    let Ok(read_dir) = fs::read_dir(root) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if scanner::is_supported_transcript_file(&path) {
            out.push(path);
        }
    }
}

/// Return up to `limit` new lines starting at `from_line` (0-indexed), plus
/// the checkpoint value to resume from next time.
///
/// The returned line count is deliberately NOT the file's true EOF line
/// count when `limit` cuts the read short — it's the index of the first
/// line not yet read. Advancing the checkpoint to true EOF regardless of
/// how much was actually read would silently skip every line past `limit`
/// forever (a real bug this signature previously had: it read at most
/// `limit` lines into the batch but always reported the file's full line
/// count as the new checkpoint).
fn read_new_lines(
    path: &Path,
    from_line: usize,
    limit: usize,
) -> Result<(Vec<(usize, String)>, usize)> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut line_no = 0usize;
    for line in reader.lines() {
        if line_no < from_line {
            line_no += 1;
            continue;
        }
        if out.len() >= limit {
            break;
        }
        let line = line.with_context(|| format!("read line from {}", path.display()))?;
        out.push((line_no, line));
        line_no += 1;
    }
    Ok((out, line_no))
}

fn codex_fallback_session_id(path: &Path, source_kind: scanner::SourceKind) -> Option<String> {
    (source_kind == scanner::SourceKind::CodexSession)
        .then(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToString::to_string)
        })
        .flatten()
}

fn seed_codex_prefix_fallbacks(
    path: &Path,
    source_kind: scanner::SourceKind,
    from_line: usize,
    fallback_project: &mut Option<String>,
    fallback_session_id: &mut Option<String>,
) {
    if source_kind != scanner::SourceKind::CodexSession
        || from_line == 0
        || (fallback_project.is_some() && fallback_session_id.is_some())
    {
        return;
    }

    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let reader = BufReader::new(file);
    let scan_limit = from_line.min(CODEX_PREFIX_METADATA_SCAN_LINES);
    for line in reader.lines().take(scan_limit).flatten() {
        scanner::update_codex_fallbacks(
            source_kind,
            line.trim_end_matches(['\r', '\n']),
            fallback_project,
            fallback_session_id,
        );
        if fallback_project.is_some() && fallback_session_id.is_some() {
            break;
        }
    }
}

async fn scan_and_forward(
    config: &AiTranscriptForwardConfig,
    client: &reqwest::Client,
    checkpoint: &mut Checkpoint,
) -> Result<usize> {
    let mut files = Vec::new();
    for root in &config.roots {
        collect_files(root, &mut files);
    }

    let mut records = Vec::new();
    let mut new_totals: HashMap<String, usize> = HashMap::new();
    for path in &files {
        // Cap the aggregate batch across ALL files, not just per-file — a
        // host with a large never-forwarded backlog (many past sessions)
        // can otherwise blow well past the server's/proxy's request-size
        // limit even with a per-file cap, since MAX_BATCH_RECORDS applied
        // per file still multiplies by however many files have new lines.
        // Files not fully drained this cycle keep their unmodified
        // checkpoint and get picked up on the next poll.
        if records.len() >= MAX_BATCH_RECORDS {
            break;
        }
        let source_kind = scanner::detect_source_kind(path);
        let key = path.to_string_lossy().to_string();
        let ai_tool = source_kind.tool_name().to_string();

        if matches!(source_kind, scanner::SourceKind::GeminiSession) {
            // Gemini sessions are a single whole-file JSON object rewritten
            // (with new messages appended) each turn, not an append-only
            // JSONL stream — there's no byte/line offset to tail. The
            // checkpoint here is a *record index* into `parse_file`'s output
            // instead: re-parse the whole file each cycle and only forward
            // records past however many were already sent. This assumes
            // Gemini never rewrites/reorders earlier messages, only appends —
            // true for an active chat session.
            let from_record = checkpoint.files.get(&key).copied().unwrap_or(0);
            let raw = match fs::read_to_string(path) {
                Ok(raw) => raw,
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = format!("{error:#}"), "ai transcript forwarder failed to read gemini file");
                    continue;
                }
            };
            let parsed = match scanner::gemini::parse_file(&raw, path) {
                Ok(parsed) => {
                    checkpoint.gemini_parse_failures.remove(&key);
                    parsed
                }
                Err(error) => {
                    if should_warn_gemini_parse_failure(checkpoint, &key, &raw) {
                        tracing::warn!(path = %path.display(), error = format!("{error:#}"), "ai transcript forwarder failed to parse gemini file");
                    }
                    continue;
                }
            };
            if parsed.missing_messages || parsed.records.len() <= from_record {
                continue;
            }
            let remaining_budget = MAX_BATCH_RECORDS - records.len();
            let new_records: Vec<_> = parsed
                .records
                .into_iter()
                .skip(from_record)
                .take(remaining_budget)
                .collect();
            // Only advance the checkpoint to how far this cycle actually
            // forwarded — if the global batch cap cut the read short, the
            // remaining tail is picked up next cycle, same as the
            // line-based sources below.
            let forwarded_through = from_record + new_records.len();
            for parsed_record in new_records {
                records.push(AiTranscriptRecord {
                    timestamp: parsed_record.timestamp,
                    hostname: config.hostname.clone(),
                    ai_tool: ai_tool.clone(),
                    ai_project: parsed_record.ai_project,
                    ai_session_id: parsed_record.session_id,
                    ai_transcript_path: key.clone(),
                    message: crate::receiver::enrichment::scrub_ai_message(
                        &parsed_record.message,
                        None,
                    ),
                });
            }
            new_totals.insert(key, forwarded_through);
            continue;
        }

        let from_line = checkpoint.files.get(&key).copied().unwrap_or(0);
        let mut fallback_project = scanner::project_for_file(source_kind, path);
        let mut fallback_session_id = codex_fallback_session_id(path, source_kind);
        seed_codex_prefix_fallbacks(
            path,
            source_kind,
            from_line,
            &mut fallback_project,
            &mut fallback_session_id,
        );
        let remaining_budget = MAX_BATCH_RECORDS - records.len();
        let (new_lines, total_lines) = match read_new_lines(path, from_line, remaining_budget) {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(path = %path.display(), error = format!("{error:#}"), "ai transcript forwarder failed to read file");
                continue;
            }
        };
        if new_lines.is_empty() {
            continue;
        }
        for (line_no, line) in &new_lines {
            scanner::update_codex_fallbacks(
                source_kind,
                line,
                &mut fallback_project,
                &mut fallback_session_id,
            );
            match scanner::parse_line_for_source(source_kind, line, path, *line_no) {
                Ok(Some(parsed)) => {
                    let ai_project = parsed
                        .ai_project
                        .as_deref()
                        .or(fallback_project.as_deref())
                        .map(normalize_local_ai_project_path);
                    let ai_session_id = parsed
                        .session_id
                        .clone()
                        .or_else(|| fallback_session_id.as_deref().map(ToString::to_string));
                    records.push(AiTranscriptRecord {
                        timestamp: parsed.timestamp,
                        hostname: config.hostname.clone(),
                        ai_tool: ai_tool.clone(),
                        ai_project,
                        ai_session_id,
                        ai_transcript_path: key.clone(),
                        message: crate::receiver::enrichment::scrub_ai_message(
                            &parsed.message,
                            None,
                        ),
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::debug!(path = %path.display(), line = line_no, error = %error, "ai transcript forwarder: unparseable line, skipping");
                }
            }
        }
        new_totals.insert(key, total_lines);
    }

    if records.is_empty() {
        return Ok(0);
    }

    let sent = records.len();
    let mut url = config.target.trim_end_matches('/').to_string();
    url.push_str("/v1/ai-transcripts");
    let mut request = client
        .post(&url)
        .json(&AiTranscriptIngestRequest { records });
    if let Some(token) = &config.token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("ai transcript POST failed")?;
    if !response.status().is_success() {
        anyhow::bail!(
            "ai transcript forward rejected: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }

    // Only advance the checkpoint after a successful forward, so a failed
    // request retries the same lines next cycle instead of losing them.
    for (key, total) in new_totals {
        checkpoint.files.insert(key, total);
    }
    save_checkpoint(&config.checkpoint_path, checkpoint)?;
    Ok(sent)
}

/// Run the AI-transcript forward loop forever, polling every
/// `config.poll_interval`. Errors from a single scan are logged and do not
/// stop the loop — matches the retry-by-continuing behavior the other agent
/// streams get from `run_agent_streams`'s outer supervision wrapper.
pub async fn run(config: AiTranscriptForwardConfig) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build ai transcript forwarder http client")?;
    let mut checkpoint = load_checkpoint(&config.checkpoint_path);
    loop {
        match scan_and_forward(&config, &client, &mut checkpoint).await {
            Ok(0) => {}
            Ok(sent) => tracing::info!(sent, "ai transcript forwarder: batch sent"),
            Err(error) => tracing::warn!(
                error = format!("{error:#}"),
                "ai transcript forward scan failed"
            ),
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}

#[cfg(test)]
#[path = "ai_transcript_tests.rs"]
mod tests;
