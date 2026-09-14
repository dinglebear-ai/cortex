//! Forwards local shell command history (zsh/bash extended history, and
//! atuin) to the central cortex server via `POST /v1/shell-history` — one
//! more supervised stream inside `cortex agent`.
//!
//! Local `cortex ingest shell user index`/`atuinindex` have zero
//! forward capability (they only ever write to whatever local SQLite file
//! the process itself has open) — this is a real gap for the same reason
//! AI-transcript and agent-command forwarding were: a host's shell activity
//! is only useful centrally if it actually reaches wherever the shared
//! server lives.
//!
//! zsh/bash history is a plain append-only text file, tailed by a persisted
//! byte offset plus line number. Atuin history is a real SQLite database
//! (`~/.local/share/atuin/history.db`), so it's polled with a `(timestamp, id)`
//! cursor query, mirroring the local `import_atuin_history_with_state`
//! approach but read-only and without any local DB write.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::command_log::{parse_zsh_extended_history_line, scrub_command};
use crate::shell_history_ingest::{ShellHistoryIngestRequest, ShellHistoryRecord};

/// Cap on the aggregate batch per scan cycle, across zsh and atuin combined —
/// stays comfortably under the server's `MAX_RECORDS_PER_BATCH` (2,000) and
/// any fronting proxy's request-size limit.
const MAX_BATCH_RECORDS: usize = 500;

/// Bytes immediately before the persisted cursor used to prove that an
/// append kept the already-consumed tail intact. This makes each poll's
/// rewrite check constant-size even when the history file is many gigabytes.
const ZSH_CURSOR_FINGERPRINT_BYTES: u64 = 4 * 1024;

#[derive(Debug, Clone)]
pub struct ShellHistoryForwardConfig {
    /// `~/.zsh_history` (extended history format), if present.
    pub zsh_history_path: Option<PathBuf>,
    /// `~/.local/share/atuin/history.db`, if present.
    pub atuin_db_path: Option<PathBuf>,
    pub target: String,
    pub token: Option<String>,
    pub hostname: String,
    pub checkpoint_path: PathBuf,
    pub poll_interval: Duration,
}

impl ShellHistoryForwardConfig {
    pub fn new(target: String, token: Option<String>, checkpoint_path: PathBuf) -> Self {
        let home = crate::env::var_os("HOME").map(PathBuf::from);
        let zsh_history_path = home.as_ref().map(|h| h.join(".zsh_history"));
        let atuin_db_path = home
            .as_ref()
            .map(|h| h.join(".local/share/atuin/history.db"));
        Self {
            zsh_history_path: zsh_history_path.filter(|p| p.exists()),
            atuin_db_path: atuin_db_path.filter(|p| p.exists()),
            target,
            token,
            hostname: crate::hostname::local_hostname(),
            checkpoint_path,
            poll_interval: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Checkpoint {
    /// Lines already forwarded from `zsh_history_path`.
    zsh_line: usize,
    /// Legacy full-prefix hash. Read during one-time checkpoint migration,
    /// then cleared; current checkpoints use the bounded cursor fingerprint.
    #[serde(default)]
    zsh_prefix_hash: String,
    #[serde(default)]
    zsh_byte_offset: u64,
    #[serde(default)]
    zsh_cursor_fingerprint: String,
    #[serde(default)]
    zsh_file_len: u64,
    #[serde(default)]
    zsh_modified_ns: u64,
    #[serde(default)]
    zsh_file_dev: u64,
    #[serde(default)]
    zsh_file_ino: u64,
    /// Atuin cursor: `(timestamp_ns, id)` of the last forwarded row.
    atuin_timestamp_ns: i64,
    #[serde(default)]
    atuin_id: String,
}

#[derive(Debug, Clone, Copy)]
struct ZshFileState {
    len: u64,
    modified_ns: u64,
    dev: u64,
    ino: u64,
}

impl ZshFileState {
    fn read(path: &std::path::Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("read metadata for {}", path.display()))?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or_default();
        let (dev, ino) = file_identity(&metadata);
        Ok(Self {
            len: metadata.len(),
            modified_ns,
            dev,
            ino,
        })
    }

    fn same_identity(self, checkpoint: &Checkpoint) -> bool {
        checkpoint.zsh_file_dev == 0
            || checkpoint.zsh_file_ino == 0
            || (self.dev == checkpoint.zsh_file_dev && self.ino == checkpoint.zsh_file_ino)
    }

    fn same_metadata(self, checkpoint: &Checkpoint) -> bool {
        self.len == checkpoint.zsh_file_len
            && self.modified_ns == checkpoint.zsh_modified_ns
            && self.same_identity(checkpoint)
    }

    fn same_file_as(self, other: Self) -> bool {
        self.dev == 0 || self.ino == 0 || (self.dev == other.dev && self.ino == other.ino)
    }
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn file_identity(_metadata: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
std::thread_local! {
    static ZSH_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn note_zsh_bytes_read(bytes: usize) {
    #[cfg(test)]
    ZSH_BYTES_READ.set(ZSH_BYTES_READ.get().saturating_add(bytes as u64));
    #[cfg(not(test))]
    let _ = bytes;
}

fn zsh_cursor_fingerprint(path: &std::path::Path, byte_offset: u64) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let start = byte_offset.saturating_sub(ZSH_CURSOR_FINGERPRINT_BYTES);
    let bytes_to_read = byte_offset - start;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; bytes_to_read as usize];
    file.read_exact(&mut bytes)
        .with_context(|| format!("read cursor fingerprint from {}", path.display()))?;
    note_zsh_bytes_read(bytes.len());
    let mut digest = Sha256::new();
    digest.update(start.to_le_bytes());
    digest.update(byte_offset.to_le_bytes());
    digest.update(&bytes);
    Ok(hex::encode(digest.finalize()))
}

struct LegacyZshPosition {
    lines: usize,
    byte_offset: u64,
    prefix_hash: String,
}

/// Translate an old line-only checkpoint once. Subsequent polls seek directly
/// to `zsh_byte_offset` and never scan the consumed prefix again.
fn migrate_legacy_zsh_position(
    path: &std::path::Path,
    requested_lines: usize,
) -> Result<LegacyZshPosition> {
    use std::io::BufRead;

    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buf = Vec::new();
    let mut lines = 0;
    let mut byte_offset = 0;
    while lines < requested_lines {
        buf.clear();
        let bytes_read = reader.read_until(b'\n', &mut buf)?;
        if bytes_read == 0 {
            break;
        }
        note_zsh_bytes_read(bytes_read);
        digest.update(&buf);
        lines += 1;
        byte_offset += bytes_read as u64;
    }
    Ok(LegacyZshPosition {
        lines,
        byte_offset,
        prefix_hash: hex::encode(digest.finalize()),
    })
}

/// Opaque stable identity for one source record. Zsh has no durable row ID,
/// so its identity combines the physical line number with the unsanitized
/// source line. An unchanged prefix replay therefore reuses the same key,
/// while a rewritten line at the same offset gets a new key. Atuin supplies
/// its own durable row ID as `source_identity`.
fn source_record_key(source: &str, source_identity: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(source.as_bytes());
    digest.update(b"\0");
    digest.update(source_identity.as_bytes());
    format!("{source}:{}", hex::encode(digest.finalize()))
}

fn load_checkpoint(path: &std::path::Path) -> Checkpoint {
    match std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(value) => value,
        None => std::fs::read(path.with_extension("json.bak"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default(),
    }
}

fn save_checkpoint(path: &std::path::Path, checkpoint: &Checkpoint) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create checkpoint dir {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec(checkpoint)?;
    crate::setup::heartbeat_agent_env::atomic_checkpoint_write(path, &bytes)
        .with_context(|| format!("failed to write checkpoint file {}", path.display()))
}

/// Read up to `limit` new zsh extended-history lines starting at `from_line`
/// (0-indexed), plus the checkpoint value to resume from next time. Mirrors
/// `agent::ai_transcript::read_new_lines`'s contract: the returned line
/// count reflects how far the (possibly limit-truncated) read actually got,
/// never the file's true EOF when the limit cuts it short.
///
/// Reads raw bytes and lossily converts to UTF-8 per line instead of
/// `BufRead::lines()` (which hard-errors the whole read on the first
/// invalid-UTF-8 byte). Real `.zsh_history` files can and do contain
/// stray non-UTF-8 bytes (pasted binary output, odd terminal escapes) —
/// one bad line must not block every line after it from ever forwarding.
fn read_new_zsh_lines(
    path: &std::path::Path,
    from_line: usize,
    from_byte_offset: u64,
    limit: usize,
) -> Result<(Vec<String>, usize, u64)> {
    use std::io::{BufRead, Seek, SeekFrom};
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    reader.seek(SeekFrom::Start(from_byte_offset))?;
    let mut out = Vec::new();
    let mut line_no = from_line;
    let mut byte_offset = from_byte_offset;
    let mut buf = Vec::new();
    while out.len() < limit {
        buf.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buf)
            .with_context(|| format!("read line from {}", path.display()))?;
        if bytes_read == 0 {
            break; // EOF
        }
        note_zsh_bytes_read(bytes_read);
        let line = String::from_utf8_lossy(&buf)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        out.push(line);
        line_no += 1;
        byte_offset += bytes_read as u64;
    }
    Ok((out, line_no, byte_offset))
}

fn scan_zsh(
    path: &std::path::Path,
    hostname: &str,
    from_line: usize,
    from_byte_offset: u64,
    limit: usize,
) -> Result<(Vec<ShellHistoryRecord>, usize, u64)> {
    let (lines, new_line, new_byte_offset) =
        read_new_zsh_lines(path, from_line, from_byte_offset, limit)?;
    let mut records = Vec::new();
    for (offset, line) in lines.iter().enumerate() {
        let Some(parsed) = parse_zsh_extended_history_line(line) else {
            continue;
        };
        records.push(ShellHistoryRecord {
            idempotency_key: Some(source_record_key(
                "zsh",
                &format!("{}\0{line}", from_line + offset),
            )),
            source: "zsh".to_string(),
            hostname: hostname.to_string(),
            timestamp: parsed
                .started_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            duration_ms: Some(parsed.duration_secs * 1000),
            command: scrub_command(&parsed.command),
            cwd: None,
            exit_status: None,
            session_id: None,
        });
    }
    Ok((records, new_line, new_byte_offset))
}

fn scan_atuin(
    path: &std::path::Path,
    hostname: &str,
    from_timestamp_ns: i64,
    from_id: &str,
    limit: usize,
) -> Result<(Vec<ShellHistoryRecord>, i64, String)> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("open atuin history {}", path.display()))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, duration, exit, command, cwd, session
             FROM history
             WHERE deleted_at IS NULL
               AND (timestamp > ?1 OR (timestamp = ?1 AND id > ?2))
             ORDER BY timestamp ASC, id ASC
             LIMIT ?3",
        )
        .context("prepare atuin history query")?;
    let mut last_timestamp_ns = from_timestamp_ns;
    let mut last_id = from_id.to_string();
    let mut records = Vec::new();
    let rows = stmt
        .query_map(
            rusqlite::params![from_timestamp_ns, from_id, limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .context("read atuin history")?;
    for row in rows {
        let (id, timestamp_ns, duration_ns, exit_status, command, cwd, session) =
            row.context("decode atuin history row")?;
        last_timestamp_ns = timestamp_ns;
        last_id = id.clone();
        let secs = timestamp_ns.div_euclid(1_000_000_000);
        let nanos = timestamp_ns.rem_euclid(1_000_000_000) as u32;
        let Some(started_at) = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos) else {
            continue;
        };
        records.push(ShellHistoryRecord {
            idempotency_key: Some(source_record_key("atuin", &id)),
            source: "atuin".to_string(),
            hostname: hostname.to_string(),
            timestamp: started_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            duration_ms: if duration_ns >= 0 {
                Some((duration_ns / 1_000_000) as u64)
            } else {
                None
            },
            command: scrub_command(&command),
            cwd: Some(cwd).filter(|s| !s.is_empty()),
            exit_status: if exit_status >= 0 {
                Some(exit_status as i32)
            } else {
                None
            },
            session_id: Some(session).filter(|s| !s.is_empty()),
        });
    }
    Ok((records, last_timestamp_ns, last_id))
}

async fn scan_and_forward(
    config: &ShellHistoryForwardConfig,
    client: &reqwest::Client,
    checkpoint: &mut Checkpoint,
) -> Result<usize> {
    let mut records = Vec::new();
    let mut new_zsh_checkpoint = None;
    let mut new_atuin_cursor = None;

    if let Some(path) = &config.zsh_history_path {
        let state = ZshFileState::read(path)?;
        let initialized = !checkpoint.zsh_cursor_fingerprint.is_empty();
        let mut from_line = checkpoint.zsh_line;
        let mut from_byte_offset = checkpoint.zsh_byte_offset;
        let mut must_scan = from_byte_offset < state.len;

        if !initialized {
            if checkpoint.zsh_line > 0 {
                let legacy = migrate_legacy_zsh_position(path, checkpoint.zsh_line)?;
                let legacy_rewritten = legacy.lines != checkpoint.zsh_line
                    || (!checkpoint.zsh_prefix_hash.is_empty()
                        && legacy.prefix_hash != checkpoint.zsh_prefix_hash);
                if legacy_rewritten {
                    from_line = 0;
                    from_byte_offset = 0;
                } else {
                    from_byte_offset = legacy.byte_offset;
                }
            }
            // Persist a bounded cursor even when the migrated file is idle.
            must_scan = true;
        } else if !state.same_identity(checkpoint) || state.len < from_byte_offset {
            from_line = 0;
            from_byte_offset = 0;
            must_scan = true;
        } else if !state.same_metadata(checkpoint) {
            // Equal-length metadata changes cannot be appends. Replay the file;
            // receipt keys make unchanged source rows idempotent while changed
            // rows at the same line receive new identities.
            if state.len == checkpoint.zsh_file_len {
                from_line = 0;
                from_byte_offset = 0;
            } else {
                // For a growing file, prove the bounded window immediately
                // before the cursor is unchanged before treating it as append.
                let fingerprint = zsh_cursor_fingerprint(path, from_byte_offset)?;
                if fingerprint != checkpoint.zsh_cursor_fingerprint {
                    from_line = 0;
                    from_byte_offset = 0;
                }
            }
            must_scan = true;
        }

        if must_scan {
            match scan_zsh(
                path,
                &config.hostname,
                from_line,
                from_byte_offset,
                MAX_BATCH_RECORDS,
            ) {
                Ok((mut zsh_records, new_line, new_byte_offset)) => {
                    let new_state = ZshFileState::read(path)?;
                    if !new_state.same_file_as(state) || new_state.len < new_byte_offset {
                        anyhow::bail!("zsh history changed while it was being scanned");
                    }
                    let cursor_fingerprint = zsh_cursor_fingerprint(path, new_byte_offset)?;
                    new_zsh_checkpoint =
                        Some((new_line, new_byte_offset, cursor_fingerprint, new_state));
                    records.append(&mut zsh_records);
                }
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = format!("{error:#}"), "shell history forwarder failed to read zsh history");
                }
            }
        }
    }

    if records.len() < MAX_BATCH_RECORDS
        && let Some(path) = &config.atuin_db_path
    {
        let remaining = MAX_BATCH_RECORDS - records.len();
        match scan_atuin(
            path,
            &config.hostname,
            checkpoint.atuin_timestamp_ns,
            &checkpoint.atuin_id,
            remaining,
        ) {
            Ok((mut atuin_records, last_ts, last_id)) => {
                if last_ts != checkpoint.atuin_timestamp_ns || last_id != checkpoint.atuin_id {
                    new_atuin_cursor = Some((last_ts, last_id));
                }
                records.append(&mut atuin_records);
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), error = format!("{error:#}"), "shell history forwarder failed to read atuin history");
            }
        }
    }

    if records.is_empty() {
        // Invalid rows still consume the raw scan budget. Persist their progress
        // without a POST so they cannot permanently hide subsequent valid rows.
        if let Some((new_line, byte_offset, cursor_fingerprint, state)) = new_zsh_checkpoint {
            checkpoint.zsh_line = new_line;
            checkpoint.zsh_prefix_hash.clear();
            checkpoint.zsh_byte_offset = byte_offset;
            checkpoint.zsh_cursor_fingerprint = cursor_fingerprint;
            checkpoint.zsh_file_len = state.len;
            checkpoint.zsh_modified_ns = state.modified_ns;
            checkpoint.zsh_file_dev = state.dev;
            checkpoint.zsh_file_ino = state.ino;
        }
        if let Some((ts, id)) = new_atuin_cursor {
            checkpoint.atuin_timestamp_ns = ts;
            checkpoint.atuin_id = id;
        }
        save_checkpoint(&config.checkpoint_path, checkpoint)?;
        return Ok(0);
    }

    let sent = records.len();
    let mut url = config.target.trim_end_matches('/').to_string();
    url.push_str("/v1/shell-history");
    let mut request = client
        .post(&url)
        .json(&ShellHistoryIngestRequest { records });
    if let Some(token) = &config.token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("shell history POST failed")?;
    if !response.status().is_success() {
        anyhow::bail!(
            "shell history forward rejected: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }

    // Only advance checkpoints after a successful forward, so a failed
    // request retries the same records next cycle instead of losing them.
    if let Some((new_line, byte_offset, cursor_fingerprint, state)) = new_zsh_checkpoint {
        checkpoint.zsh_line = new_line;
        checkpoint.zsh_prefix_hash.clear();
        checkpoint.zsh_byte_offset = byte_offset;
        checkpoint.zsh_cursor_fingerprint = cursor_fingerprint;
        checkpoint.zsh_file_len = state.len;
        checkpoint.zsh_modified_ns = state.modified_ns;
        checkpoint.zsh_file_dev = state.dev;
        checkpoint.zsh_file_ino = state.ino;
    }
    if let Some((ts, id)) = new_atuin_cursor {
        checkpoint.atuin_timestamp_ns = ts;
        checkpoint.atuin_id = id;
    }
    save_checkpoint(&config.checkpoint_path, checkpoint)?;
    Ok(sent)
}

/// Run the shell-history forward loop forever, polling every
/// `config.poll_interval`. Errors from a single scan are logged and do not
/// stop the loop.
pub async fn run(config: ShellHistoryForwardConfig) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build shell history forwarder http client")?;
    let mut checkpoint = load_checkpoint(&config.checkpoint_path);
    loop {
        match scan_and_forward(&config, &client, &mut checkpoint).await {
            Ok(0) => {}
            Ok(sent) => tracing::info!(sent, "shell history forwarder: batch sent"),
            Err(error) => tracing::warn!(
                error = format!("{error:#}"),
                "shell history forward scan failed"
            ),
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}

#[cfg(test)]
#[path = "shell_history_tests.rs"]
mod tests;
