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

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ai_project::normalize_local_ai_project_path;
use crate::ai_transcript_ingest::{
    AiTranscriptIngestRequest, AiTranscriptIngestResponse, AiTranscriptRecord,
    EVIDENCE_ENVELOPE_VERSION, EvidenceCapabilityCoverage, EvidenceCoverage, EvidenceEnvelope,
    EvidenceSource,
};
use crate::scanner;

/// Cap on the *aggregate* batch per scan cycle, across every transcript file
/// combined — stays comfortably under the server's `MAX_RECORDS_PER_BATCH`
/// (2,000) and any fronting proxy's request-size limit. A backlog larger
/// than this drains over several poll cycles instead of one oversized POST.
const MAX_BATCH_RECORDS: usize = 500;
const CODEX_PREFIX_METADATA_SCAN_LINES: usize = 200;
const TRANSCRIPT_FORWARDER_ADAPTER_VERSION: &str = "cortex-ai-forwarder-v1";

#[derive(Debug, Clone)]
pub struct AiTranscriptForwardConfig {
    pub roots: Vec<PathBuf>,
    /// Central server base URL, e.g. `http://nashost:3100`.
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
    /// Bounded source-prefix fingerprints. A changed prefix resets that
    /// file's local cursor; receipt IDs keep the replay safe.
    #[serde(default)]
    fingerprints: HashMap<String, String>,
    /// In-process record of malformed Gemini transcripts, keyed by canonical
    /// path. Deliberately not persisted: a restart should re-warn rather than
    /// inherit suppression from a previous process.
    ///
    /// Suppression is bounded in both directions. Warning on every poll cycle
    /// floods journald, but warning only once per content revision lets a file
    /// that goes malformed and then stops changing — a truncated write, a
    /// crashed session, on-disk corruption — go silent for the lifetime of the
    /// agent while its data is never forwarded. So a warning repeats when the
    /// content changes *or* when [`GEMINI_REWARN_INTERVAL`] has elapsed.
    #[serde(skip)]
    gemini_parse_failures: HashMap<String, GeminiParseFailure>,
}

/// How long a persistently malformed Gemini transcript stays quiet between
/// warnings. Long enough not to be noise, short enough that an operator
/// scanning a day of logs cannot miss it.
const GEMINI_REWARN_INTERVAL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone)]
struct GeminiParseFailure {
    fingerprint: u64,
    last_warned: Instant,
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
    let tmp_path = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp_path).with_context(|| {
        format!(
            "failed to create checkpoint temp file {}",
            tmp_path.display()
        )
    })?;
    file.write_all(&bytes).with_context(|| {
        format!(
            "failed to write checkpoint temp file {}",
            tmp_path.display()
        )
    })?;
    file.sync_all()
        .with_context(|| format!("failed to sync checkpoint temp file {}", tmp_path.display()))?;
    drop(file);
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to atomically replace checkpoint file {}",
            path.display()
        )
    })
}

fn gemini_content_fingerprint(raw: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    hasher.finish()
}

/// Returns true when this parse failure should be logged: the content changed
/// since the last warning, or the re-warn interval has elapsed. `now` is a
/// parameter so tests can advance the clock without sleeping.
fn should_warn_gemini_parse_failure(
    checkpoint: &mut Checkpoint,
    key: &str,
    raw: &str,
    now: Instant,
) -> bool {
    let fingerprint = gemini_content_fingerprint(raw);
    let warn = match checkpoint.gemini_parse_failures.get(key) {
        Some(previous) => {
            previous.fingerprint != fingerprint
                || now.duration_since(previous.last_warned) >= GEMINI_REWARN_INTERVAL
        }
        None => true,
    };
    if warn {
        checkpoint.gemini_parse_failures.insert(
            key.to_string(),
            GeminiParseFailure {
                fingerprint,
                last_warned: now,
            },
        );
    }
    warn
}

/// Drop records for transcripts that no longer exist, so a long-lived agent
/// with rotating sessions does not accumulate entries for deleted files.
fn evict_missing_gemini_failures(checkpoint: &mut Checkpoint, present: &HashSet<String>) {
    checkpoint
        .gemini_parse_failures
        .retain(|key, _| present.contains(key));
}

/// Recursively collect supported transcript files under `root` (mirrors
/// `scanner`'s discovery rules via the public `is_supported_transcript_file`
/// predicate, without pulling in the local-indexing `IndexResult` coupling
/// that `scanner::collect_supported_files` carries).
fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    if fs::symlink_metadata(root)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        // Forwarder roots are local operator configuration, but their
        // descendants are still hostile filesystem input. Never traverse a
        // symlink into an unrelated home/cache tree or forward its locator.
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
        if fs::symlink_metadata(&path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out);
        } else if scanner::is_supported_transcript_file(&path) {
            out.push(path);
        }
    }
}

/// The scanner's provider registry intentionally recognizes only configured
/// `$HOME` roots. An agent may be explicitly configured with a mounted or
/// test root outside that home, though, so preserve provider classification
/// from the safe, structural transcript layout as a fallback. This never
/// broadens file discovery: [`collect_files`] already admits only supported
/// filename shapes.
fn forward_source_kind(path: &Path) -> scanner::SourceKind {
    match scanner::detect_source_kind(path) {
        scanner::SourceKind::ExplicitFile if scanner::gemini::is_chat_file(path) => {
            scanner::SourceKind::GeminiSession
        }
        scanner::SourceKind::ExplicitFile => {
            scanner::providers::provider_for_transcript_layout(path)
                .and_then(scanner::providers::source_kind_for_provider)
                .and_then(scanner::SourceKind::from_persisted_kind)
                .unwrap_or(scanner::SourceKind::ExplicitFile)
        }
        source_kind => source_kind,
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

fn sha256_id(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_ref());
        hasher.update([0]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn source_epoch(path: &Path) -> String {
    let mut parts = Vec::new();
    if let Ok(metadata) = fs::metadata(path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            parts.push(metadata.dev().to_le_bytes().to_vec());
            parts.push(metadata.ino().to_le_bytes().to_vec());
        }
        parts.push(
            metadata
                .created()
                .ok()
                .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos().to_le_bytes().to_vec())
                .unwrap_or_default(),
        );
    } else {
        // A disappeared source cannot be forwarded, but retain a deterministic
        // fallback for callers constructing an envelope during a race.
        parts.push(path.as_os_str().as_encoded_bytes().to_vec());
    }
    sha256_id(parts)
}

fn file_prefix_fingerprint(path: &Path) -> Result<String> {
    use std::io::Read;

    let mut file = fs::File::open(path)?;
    let mut bytes = vec![0; 4096];
    let read = file.read(&mut bytes)?;
    bytes.truncate(read);
    Ok(sha256_id([bytes.as_slice()]))
}

/// Returns a fingerprint for the acknowledged logical prefix of a Gemini
/// transcript. Gemini persists an entire JSON snapshot on every turn, so a
/// byte prefix changes even when the already-forwarded messages have not.
/// Fingerprinting the parsed records instead lets an append retain its record
/// cursor while still detecting a rewrite, reordering, or truncation of the
/// acknowledged prefix.
fn gemini_prefix_fingerprint(records: &[scanner::ParsedTranscriptRecord], count: usize) -> String {
    let mut hasher = Sha256::new();
    for record in records.iter().take(count) {
        for field in [
            record.record_key.as_bytes(),
            record.timestamp.as_deref().unwrap_or_default().as_bytes(),
            record.event_kind.as_bytes(),
            record.message.as_bytes(),
            record.session_id.as_deref().unwrap_or_default().as_bytes(),
        ] {
            hasher.update(field);
            hasher.update([0]);
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn capability_coverage(source_kind: scanner::SourceKind) -> EvidenceCapabilityCoverage {
    let provider = scanner::providers::provider_for_source_kind(source_kind.as_str());
    let coverage = |lane| {
        provider
            .map(|provider| scanner::providers::definition(provider).forwarding_coverage(lane))
            // An explicitly configured generic file remains a bounded,
            // parseable transcript input, but never inherits provider events.
            .unwrap_or_else(|| match lane {
                scanner::providers::ProviderLane::Transcript => {
                    scanner::providers::Coverage::Partial
                }
                _ => scanner::providers::Coverage::NotObserved,
            })
    };

    EvidenceCapabilityCoverage {
        transcript: evidence_coverage(coverage(scanner::providers::ProviderLane::Transcript)),
        mcp_events: evidence_coverage(coverage(scanner::providers::ProviderLane::McpEvents)),
        skill_events: evidence_coverage(coverage(scanner::providers::ProviderLane::Skills)),
        hook_events: evidence_coverage(coverage(scanner::providers::ProviderLane::Hooks)),
    }
}

fn evidence_coverage(coverage: scanner::providers::Coverage) -> EvidenceCoverage {
    match coverage {
        scanner::providers::Coverage::Observed => EvidenceCoverage::Observed,
        scanner::providers::Coverage::Partial => EvidenceCoverage::Partial,
        scanner::providers::Coverage::NotObserved => EvidenceCoverage::NotObserved,
        scanner::providers::Coverage::Failed => EvidenceCoverage::Failed,
    }
}

fn safe_provenance_id(prefix: &str, value: Option<String>) -> Option<String> {
    value.map(|value| format!("{prefix}:{}", sha256_id([value.as_bytes()])))
}

struct TranscriptRecordDetails {
    revision: String,
    timestamp: Option<String>,
    ai_project: Option<String>,
    ai_session_id: Option<String>,
    event_kind: Option<String>,
    message: String,
}

fn transcript_record(
    config: &AiTranscriptForwardConfig,
    path: &Path,
    source_kind: scanner::SourceKind,
    details: TranscriptRecordDetails,
) -> AiTranscriptRecord {
    let provider = source_kind.tool_name().to_string();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let canonical_bytes = canonical.as_os_str().as_encoded_bytes();
    let source_identity = match details.ai_session_id.as_deref() {
        // A native provider session identifier survives an archive move. It is
        // hashed before forwarding, so the canonical receipt contains no raw
        // session text or local path.
        Some(session_id) => sha256_id([
            b"ai-transcript-native-session",
            provider.as_bytes(),
            config.hostname.as_bytes(),
            session_id.as_bytes(),
        ]),
        None => sha256_id([provider.as_bytes(), canonical_bytes]),
    };
    let epoch = source_epoch(&canonical);
    let source_revision = sha256_id([details.revision.as_bytes()]);
    let locator = sha256_id([
        b"ai-transcript-locator",
        provider.as_bytes(),
        canonical_bytes,
    ]);
    let source_record_id = sha256_id([
        b"ai-transcript-record-v1",
        provider.as_bytes(),
        source_identity.as_bytes(),
        epoch.as_bytes(),
        source_revision.as_bytes(),
    ]);
    AiTranscriptRecord {
        envelope: EvidenceEnvelope {
            version: EVIDENCE_ENVELOPE_VERSION,
            source_record_id,
            source: EvidenceSource {
                provider,
                adapter_version: TRANSCRIPT_FORWARDER_ADAPTER_VERSION.to_string(),
                source_identity,
                source_epoch: epoch,
                source_revision,
                locator,
                native_session_id: safe_provenance_id("session", details.ai_session_id.clone()),
                title: None,
            },
            timestamp: details.timestamp,
            hostname: config.hostname.clone(),
            ai_project: safe_provenance_id("project", details.ai_project),
            ai_session_id: safe_provenance_id("session", details.ai_session_id),
            event_kind: details.event_kind,
            message: crate::receiver::enrichment::scrub_ai_message(&details.message, None),
            capabilities: capability_coverage(source_kind),
            diagnostics: Vec::new(),
        },
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
    evict_missing_gemini_failures(
        checkpoint,
        &files
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    );

    let mut records = Vec::new();
    let mut new_totals: HashMap<String, usize> = HashMap::new();
    let mut new_fingerprints: HashMap<String, String> = HashMap::new();
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
        let source_kind = forward_source_kind(path);
        let key = path.to_string_lossy().to_string();

        if matches!(source_kind, scanner::SourceKind::GeminiSession) {
            // Gemini sessions are a single whole-file JSON object rewritten
            // (with new messages appended) each turn, not an append-only
            // JSONL stream — there's no byte/line offset to tail. The
            // checkpoint here is a *record index* into `parse_file`'s output
            // instead: re-parse the whole file each cycle and only forward
            // records past however many were already sent. The fingerprint is
            // of that acknowledged logical record prefix, rather than raw
            // bytes: every normal Gemini append rewrites the whole JSON file.
            let mut from_record = checkpoint.files.get(&key).copied().unwrap_or(0);
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
                    if should_warn_gemini_parse_failure(checkpoint, &key, &raw, Instant::now()) {
                        tracing::warn!(
                            path = %path.display(),
                            error = format!("{error:#}"),
                            unparseable_transcripts = checkpoint.gemini_parse_failures.len(),
                            "ai transcript forwarder failed to parse gemini file — its data is not being forwarded"
                        );
                    }
                    continue;
                }
            };
            if parsed.records.len() < from_record
                || (from_record > 0
                    && checkpoint.fingerprints.get(&key).is_some_and(|previous| {
                        previous != &gemini_prefix_fingerprint(&parsed.records, from_record)
                    }))
            {
                // A shrink or a changed logical prefix means this is a source
                // rewrite/rotation. Replay from zero; exact server receipts
                // suppress unchanged records.
                from_record = 0;
            }
            if parsed.missing_messages || parsed.records.len() <= from_record {
                continue;
            }
            let remaining_budget = MAX_BATCH_RECORDS - records.len();
            let forwarded_through = (from_record + remaining_budget).min(parsed.records.len());
            let fingerprint = gemini_prefix_fingerprint(&parsed.records, forwarded_through);
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
            for (record_index, parsed_record) in new_records.into_iter().enumerate() {
                let revision = format!(
                    "gemini:{}:{}:{}",
                    from_record + record_index,
                    parsed_record.event_kind,
                    parsed_record.message
                );
                records.push(transcript_record(
                    config,
                    path,
                    source_kind,
                    TranscriptRecordDetails {
                        revision,
                        timestamp: parsed_record.timestamp,
                        ai_project: parsed_record.ai_project,
                        ai_session_id: parsed_record.session_id,
                        event_kind: Some(parsed_record.event_kind),
                        message: parsed_record.message,
                    },
                ));
            }
            new_totals.insert(key, forwarded_through);
            new_fingerprints.insert(path.to_string_lossy().to_string(), fingerprint);
            continue;
        }

        let mut from_line = checkpoint.files.get(&key).copied().unwrap_or(0);
        let fingerprint = match file_prefix_fingerprint(path) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "ai transcript forwarder failed to fingerprint file");
                continue;
            }
        };
        if checkpoint
            .fingerprints
            .get(&key)
            .is_some_and(|previous| previous != &fingerprint)
        {
            from_line = 0;
        }
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
                    records.push(transcript_record(
                        config,
                        path,
                        source_kind,
                        TranscriptRecordDetails {
                            revision: format!("line:{line_no}:{line}"),
                            timestamp: parsed.timestamp,
                            ai_project,
                            ai_session_id,
                            event_kind: Some(parsed.event_kind),
                            message: parsed.message,
                        },
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::debug!(path = %path.display(), line = line_no, error = %error, "ai transcript forwarder: unparseable line, skipping");
                }
            }
        }
        new_totals.insert(key, total_lines);
        new_fingerprints.insert(path.to_string_lossy().to_string(), fingerprint);
    }

    if records.is_empty() {
        return Ok(0);
    }

    let sent = records.len();
    let expected_receipts: HashSet<String> = records
        .iter()
        .map(|record| record.envelope.source_record_id.clone())
        .collect();
    anyhow::ensure!(
        expected_receipts.len() == sent,
        "ai transcript forward constructed duplicate source-record identities"
    );
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
    let receipt: AiTranscriptIngestResponse = response
        .json()
        .await
        .context("ai transcript forward response was not a receipt")?;
    let returned_receipts: HashSet<String> = receipt
        .receipts
        .iter()
        .map(|receipt| receipt.source_record_id.clone())
        .collect();
    if receipt.accepted != sent
        || receipt.receipts.len() != sent
        || returned_receipts != expected_receipts
    {
        anyhow::bail!(
            "ai transcript forward returned incomplete receipt set: expected {sent}, accepted {}, receipts {}",
            receipt.accepted,
            receipt.receipts.len()
        );
    }

    // Only advance after the server supplied an exact receipt for every
    // submitted source-record ID. A lost/malformed response leaves the local
    // cursor untouched; a retry is deduplicated by the server receipt table.
    for (key, total) in new_totals {
        checkpoint.files.insert(key, total);
    }
    checkpoint.fingerprints.extend(new_fingerprints);
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
