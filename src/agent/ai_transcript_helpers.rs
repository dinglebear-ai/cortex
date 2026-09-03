//! Checkpoint, discovery, and record-normalization helpers.

use super::*;

#[path = "ai_transcript_helpers_checkpoint.rs"]
mod checkpoint;
pub(super) use checkpoint::*;

/// Recursively collect supported transcript files under `root` (mirrors
/// `scanner`'s discovery rules via the public `is_supported_transcript_file`
/// predicate, without pulling in the local-indexing `IndexResult` coupling
/// that `scanner::collect_supported_files` carries).
pub(super) fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    collect_files_after(root, out, None);
}

/// Collect one stable discovery window after `after`. Directories are always
/// traversed, because a directory whose own path precedes the cursor can
/// contain later files. File paths are sorted at every level so a persisted
/// cursor rotates predictably across a large transcript tree.
pub(super) fn collect_files_after(root: &Path, out: &mut Vec<PathBuf>, after: Option<&Path>) {
    if out.len() >= MAX_FORWARD_FILES {
        return;
    }
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
        if scanner::is_supported_transcript_file(root)
            && after.is_none_or(|cursor| root > cursor)
            && (!scanner::gemini::is_chat_file(root)
                || root
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() <= MAX_TRANSCRIPT_FILE_BYTES))
        {
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
    let mut paths = read_dir
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if fs::symlink_metadata(&path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            continue;
        }
        if path.is_dir() {
            collect_files_after(&path, out, after);
        } else if scanner::is_supported_transcript_file(&path)
            && after.is_none_or(|cursor| path > cursor)
            && (!scanner::gemini::is_chat_file(&path)
                || path
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() <= MAX_TRANSCRIPT_FILE_BYTES))
        {
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
pub(super) fn forward_source_kind(path: &Path) -> scanner::SourceKind {
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

/// Return up to `limit` complete newline-terminated records starting at `from_line` (0-indexed), plus
/// the checkpoint value to resume from next time.
///
/// The returned line count is deliberately NOT the file's true EOF line
/// count when `limit` cuts the read short — it's the index of the first
/// line not yet read. Advancing the checkpoint to true EOF regardless of
/// how much was actually read would silently skip every line past `limit`
/// forever (a real bug this signature previously had: it read at most
/// `limit` lines into the batch but always reported the file's full line
/// count as the new checkpoint).
pub(super) fn read_new_lines(
    path: &Path,
    from_line: usize,
    limit: usize,
) -> Result<(Vec<(usize, String)>, usize)> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut line_no = 0usize;
    while let Some(line) = read_bounded_jsonl_line(&mut reader)? {
        if line_no < from_line {
            line_no += 1;
            continue;
        }
        if out.len() >= limit {
            break;
        }
        out.push((line_no, line));
        line_no += 1;
    }
    Ok((out, line_no))
}

pub(super) fn read_bounded_jsonl_line(reader: &mut BufReader<fs::File>) -> Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            // A writer may have appended only part of its final JSONL record.
            // It is not a record until the terminating newline arrives: handing
            // it to the caller would let a successful earlier batch advance the
            // line checkpoint past evidence that will be completed next poll.
            return Ok(None);
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_JSONL_LINE_BYTES {
            reader.consume(take);
            bail!("transcript line exceeds {MAX_JSONL_LINE_BYTES} bytes");
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.ends_with(b"\n") {
            line.pop();
            if line.ends_with(b"\r") {
                line.pop();
            }
            return String::from_utf8(line)
                .context("transcript line is not UTF-8")
                .map(Some);
        }
    }
}

pub(super) fn codex_fallback_session_id(
    path: &Path,
    source_kind: scanner::SourceKind,
) -> Option<String> {
    (source_kind == scanner::SourceKind::CodexSession)
        .then(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(ToString::to_string)
        })
        .flatten()
}

pub(super) fn seed_codex_prefix_fallbacks(
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

pub(super) fn sha256_id(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_ref());
        hasher.update([0]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn source_epoch(path: &Path) -> String {
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

pub(super) fn file_prefix_fingerprint(path: &Path, acknowledged_lines: usize) -> Result<String> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    for _ in 0..acknowledged_lines {
        let Some(line) = read_bounded_jsonl_line(&mut reader)? else {
            break;
        };
        hasher.update(line.as_bytes());
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Returns a fingerprint for the acknowledged logical prefix of a Gemini
/// transcript. Gemini persists an entire JSON snapshot on every turn, so a
/// byte prefix changes even when the already-forwarded messages have not.
/// Fingerprinting the parsed records instead lets an append retain its record
/// cursor while still detecting a rewrite, reordering, or truncation of the
/// acknowledged prefix.
pub(super) fn gemini_prefix_fingerprint(
    records: &[scanner::ParsedTranscriptRecord],
    count: usize,
) -> String {
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

pub(super) fn capability_coverage(source_kind: scanner::SourceKind) -> EvidenceCapabilityCoverage {
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

pub(super) fn evidence_coverage(coverage: scanner::providers::Coverage) -> EvidenceCoverage {
    match coverage {
        scanner::providers::Coverage::Observed => EvidenceCoverage::Observed,
        scanner::providers::Coverage::Partial => EvidenceCoverage::Partial,
        scanner::providers::Coverage::NotObserved => EvidenceCoverage::NotObserved,
        scanner::providers::Coverage::Failed => EvidenceCoverage::Failed,
    }
}

pub(super) fn safe_provenance_id(prefix: &str, value: Option<String>) -> Option<String> {
    value.map(|value| format!("{prefix}:{}", sha256_id([value.as_bytes()])))
}

pub(super) struct TranscriptRecordDetails {
    pub(super) revision: String,
    pub(super) timestamp: Option<String>,
    pub(super) ai_project: Option<String>,
    pub(super) ai_session_id: Option<String>,
    pub(super) event_kind: Option<String>,
    pub(super) message: String,
}

pub(super) fn transcript_record(
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
            timestamp: details
                .timestamp
                .map(|timestamp| truncate_utf8(&timestamp, MAX_FORWARDED_TIMESTAMP_BYTES)),
            hostname: truncate_utf8(&config.hostname, MAX_FORWARDED_IDENTIFIER_BYTES),
            ai_project: safe_provenance_id("project", details.ai_project),
            ai_session_id: safe_provenance_id("session", details.ai_session_id),
            event_kind: details
                .event_kind
                .map(|event_kind| truncate_utf8(&event_kind, MAX_FORWARDED_IDENTIFIER_BYTES)),
            message: crate::receiver::enrichment::scrub_ai_message(
                &truncate_utf8(&details.message, MAX_FORWARDED_MESSAGE_BYTES),
                None,
            ),
            capabilities: capability_coverage(source_kind),
            diagnostics: Vec::new(),
        },
    }
}

pub(super) fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}
