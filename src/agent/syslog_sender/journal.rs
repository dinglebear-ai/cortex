//! Enqueues append one synced command; snapshots compact the journal by sequence.
use super::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const COMPACT_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SourceKeyFormat {
    #[default]
    Raw,
    Sha256Prefix96,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    #[serde(default)]
    source_key_format: SourceKeyFormat,
    sequence: u64,
    at: chrono::DateTime<Utc>,
    records: Vec<(String, String)>,
}
fn path(snapshot: &Path) -> PathBuf {
    let mut name = snapshot.as_os_str().to_owned();
    name.push(".enqueue-journal");
    PathBuf::from(name)
}
fn apply(spool: &mut SpoolState, entry: Entry) -> Result<()> {
    if entry.sequence <= spool.journal_sequence {
        return Ok(());
    }
    anyhow::ensure!(
        entry.sequence == spool.journal_sequence + 1,
        "non-contiguous enqueue journal"
    );
    if matches!(entry.source_key_format, SourceKeyFormat::Sha256Prefix96) {
        anyhow::ensure!(
            entry.records.iter().all(|(source, _)| {
                source.strip_prefix("source-").is_some_and(|digest| {
                    digest.len() == 24 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            }),
            "invalid canonical journal source key"
        );
    }
    for (source, line) in entry.records {
        let canonical = match entry.source_key_format {
            SourceKeyFormat::Raw => stable_source_key(&source),
            SourceKeyFormat::Sha256Prefix96 => source,
        };
        persistence::enqueue_canonical_at(spool, canonical, line, entry.at);
    }
    spool.journal_sequence = entry.sequence;
    Ok(())
}
pub(super) fn replay(snapshot: &Path, spool: &mut SpoolState) -> Result<()> {
    let bytes = match std::fs::read(path(snapshot)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    // A crash can leave an unacknowledged trailing partial entry. Complete
    // entries must parse; never silently skip durable commands.
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        apply(spool, serde_json::from_slice(line)?)?;
    }
    Ok(())
}
pub(super) fn append(state: &mut SenderState, records: Vec<(String, String)>) -> Result<()> {
    append_with_sync(state, records, |file| file.sync_all())
}

pub(super) fn append_with_sync(
    state: &mut SenderState,
    records: Vec<(String, String)>,
    sync: impl FnOnce(&File) -> std::io::Result<()>,
) -> Result<()> {
    anyhow::ensure!(
        !state.spool.journal_failed,
        "enqueue journal requires operator recovery"
    );
    if !state.spool_path.exists() {
        save_spool(&state.spool_path, &state.spool)?;
    }
    anyhow::ensure!(
        state.spool_path.is_file(),
        "syslog snapshot is not a regular file"
    );
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path(&state.spool_path))?;
    let mut length = file.metadata()?.len();
    if length >= COMPACT_BYTES {
        save_spool(&state.spool_path, &state.spool)?;
        file.set_len(0)?;
        file.sync_all()?;
        length = 0;
    }
    // Usually reads one byte. Only a torn write requires walking back to the
    // last complete durable entry; truncate it before accepting another.
    while length > 0 {
        file.seek(SeekFrom::Start(length - 1))?;
        let mut byte = [0];
        file.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        length -= 1;
    }
    file.set_len(length)?;
    file.seek(SeekFrom::Start(length))?;
    let entry = Entry {
        source_key_format: SourceKeyFormat::Sha256Prefix96,
        sequence: state
            .spool
            .journal_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow!("enqueue journal sequence exhausted"))?,
        at: Utc::now(),
        records: records
            .into_iter()
            .map(|(source, line)| {
                let line = if line.len() > MAX_FORWARD_RECORD_BYTES {
                    " ".repeat(MAX_FORWARD_RECORD_BYTES + 1)
                } else {
                    line
                };
                (stable_source_key(&source), line)
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec(&entry)?;
    bytes.push(b'\n');
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        sync(&file)?;
        #[cfg(unix)]
        if length == 0
            && let Some(parent) = state.spool_path.parent()
        {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        if file.set_len(length).and_then(|_| file.sync_all()).is_err() {
            state.spool.journal_failed = true;
        }
        return Err(error);
    }
    apply(&mut state.spool, entry)
}
