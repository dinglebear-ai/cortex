//! Shared bounded framing and rotation handling for host-local agent files.
use crate::filetail::platform::file_identity;
use crate::filetail::supervisor::read_bounded_line;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::BufReader;

pub(super) const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TailCursor {
    identity: (u64, u64),
    position: u64,
    prefix: Vec<u8>,
    partial: Vec<u8>,
    truncated: bool,
    #[serde(default)]
    boundary: Vec<u8>,
}

pub(super) struct TailReader {
    path: PathBuf,
    reader: BufReader<tokio::fs::File>,
    identity: (u64, u64),
    position: u64,
    prefix: Vec<u8>,
    partial: Vec<u8>,
    truncated: bool,
    boundary: Vec<u8>,
    /// Offset to which the last captured guard belongs; reads may advance
    /// position before the caller requests another durable cursor.
    boundary_position: u64,
    report_gaps: bool,
    gap: Option<String>,
}

impl TailReader {
    pub(super) async fn open(path: &Path) -> Result<Self> {
        let path = path.to_owned();
        let (file, identity, position, prefix) = snapshot(path.clone(), true).await?;
        Ok(Self {
            path,
            reader: BufReader::new(tokio::fs::File::from_std(file)),
            identity,
            position,
            prefix,
            partial: Vec::new(),
            truncated: false,
            boundary: Vec::new(),
            boundary_position: position,
            report_gaps: false,
            gap: None,
        })
    }

    /// First installation deliberately follows only new records. Restarts use
    /// `restore` instead, including a bounded search for the rotated old inode.
    pub(super) async fn open_durable(path: &Path) -> Result<Self> {
        let mut reader = Self::open(path).await?;
        reader.report_gaps = true;
        Ok(reader)
    }

    pub(super) async fn cursor(&mut self) -> Result<TailCursor> {
        // Positional reads on a cloned descriptor do not disturb buffered framing.
        let file = self.reader.get_ref().try_clone().await?.into_std().await;
        let position = self.position;
        self.boundary =
            tokio::task::spawn_blocking(move || read_boundary(&file, position)).await??;
        self.boundary_position = position;
        Ok(TailCursor {
            identity: self.identity,
            position: self.position,
            prefix: self.prefix.clone(),
            partial: self.partial.clone(),
            truncated: self.truncated,
            boundary: self.boundary.clone(),
        })
    }

    pub(super) async fn restore(path: &Path, cursor: TailCursor) -> Result<Self> {
        anyhow::ensure!(
            cursor.partial.len() <= MAX_LINE_BYTES
                && cursor.prefix.len() <= 256
                && cursor.boundary.len() <= 256,
            "file-tail checkpoint exceeds bounded cursor budget"
        );
        let path = path.to_owned();
        let source = path.clone();
        let (file, identity, prefix, recovered, cursor) = tokio::task::spawn_blocking(move || {
            use std::io::{Seek, SeekFrom};
            let resumable = |candidate: &Path| -> Result<Option<ResumedSnapshot>> {
                let Ok((mut file, identity, length, prefix)) = snapshot_sync(candidate, false)
                else {
                    return Ok(None);
                };
                if identity == cursor.identity
                    && length >= cursor.position
                    && prefix.starts_with(&cursor.prefix)
                    && (cursor.boundary.is_empty()
                        || read_boundary(&file, cursor.position)? == cursor.boundary)
                {
                    file.seek(SeekFrom::Start(cursor.position))?;
                    Ok(Some((file, identity, prefix)))
                } else {
                    Ok(None)
                }
            };
            if let Some((file, identity, prefix)) = resumable(&source)? {
                return Ok::<_, anyhow::Error>((file, identity, prefix, true, cursor));
            }
            if let Some(parent) = source.parent()
                && let Ok(entries) = std::fs::read_dir(parent)
            {
                // Rotation recovery is bounded; absence becomes explicit gap evidence.
                for entry in entries.take(128).flatten() {
                    if entry.file_type().is_ok_and(|kind| kind.is_file())
                        && entry.path() != source
                        && let Some((file, identity, prefix)) = resumable(&entry.path())?
                    {
                        return Ok((file, identity, prefix, true, cursor));
                    }
                }
            }
            let (file, identity, _, prefix) = snapshot_sync(&source, false)
                .context("file-tail source unavailable during checkpoint recovery")?;
            Ok((file, identity, prefix, false, cursor))
        })
        .await??;
        // Move the saved framing bytes only when the acknowledged source survives.
        Ok(Self {
            path,
            reader: BufReader::new(tokio::fs::File::from_std(file)),
            identity,
            position: if recovered { cursor.position } else { 0 },
            prefix,
            partial: if recovered { cursor.partial } else { Vec::new() },
            truncated: recovered && cursor.truncated,
            boundary: if recovered { cursor.boundary } else { Vec::new() },
            boundary_position: if recovered { cursor.position } else { 0 },
            report_gaps: true,
            gap: (!recovered).then(|| "[Cortex file-tail gap: checkpoint source was rotated away, truncated, or rewritten; unread history may be missing]".to_owned()),
        })
    }

    fn take_record(&mut self) -> String {
        let line = if self.truncated {
            // Do not present an incomplete log as original evidence or forward
            // a prefix that may cut through a credential/redaction boundary.
            "[Cortex omitted oversized file-tail record: exceeds 65536-byte limit]".to_owned()
        } else {
            String::from_utf8_lossy(&self.partial)
                .trim_end_matches(['\r', '\n'])
                .to_owned()
        };
        self.partial.clear();
        self.truncated = false;
        line
    }

    /// None means temporary EOF; callers may sleep before polling again.
    pub(super) async fn next_line(&mut self) -> Result<Option<String>> {
        if let Some(gap) = self.gap.take() {
            return Ok(Some(gap));
        }
        // A copytruncate can regrow beyond our old offset between polls.
        // Check the same inode's prefix before reading a suffix at that offset.
        if (self.report_gaps || self.reader.buffer().is_empty())
            && let Ok((file, identity, length, prefix)) = snapshot(self.path.clone(), false).await
            && identity == self.identity
            && (length < self.position
                || !prefix.starts_with(&self.prefix)
                || !boundary_matches(&file, self.boundary_position, &self.boundary).await?)
        {
            self.reader = BufReader::new(tokio::fs::File::from_std(file));
            self.position = 0;
            self.prefix = prefix;
            self.boundary.clear();
            self.boundary_position = 0;
            if self.report_gaps {
                self.partial.clear();
                self.truncated = false;
                return Ok(Some("[Cortex file-tail gap: source truncated or rewritten; unread history may be missing]".to_owned()));
            }
            if !self.partial.is_empty() {
                return Ok(Some(self.take_record()));
            }
        }
        let read = read_bounded_line(&mut self.reader, &mut self.partial, MAX_LINE_BYTES).await?;
        self.position = self.position.saturating_add(read.bytes_read as u64);
        self.truncated |= read.truncated;
        if read.complete {
            return Ok(Some(self.take_record()));
        }
        if read.bytes_read != 0 {
            return Ok(None);
        }
        let (file, identity, length, prefix) = match snapshot(self.path.clone(), false).await {
            Ok(snapshot) => snapshot,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if identity != self.identity || length < self.position || !prefix.starts_with(&self.prefix)
        {
            // The old handle has been drained. A replacement starts at zero,
            // while the initial open alone uses follow-only EOF semantics.
            self.reader = BufReader::new(tokio::fs::File::from_std(file));
            self.identity = identity;
            self.boundary.clear();
            self.boundary_position = 0;
            self.position = 0;
            self.prefix = prefix;
            // Flush an unterminated old record once, never join it to the new file.
            if !self.partial.is_empty() {
                return Ok(Some(self.take_record()));
            }
        } else if prefix.len() > self.prefix.len() {
            self.prefix = prefix;
        }
        Ok(None)
    }
}

type ResumedSnapshot = (std::fs::File, (u64, u64), Vec<u8>);
type Snapshot = (std::fs::File, (u64, u64), u64, Vec<u8>);
async fn snapshot(path: PathBuf, at_end: bool) -> Result<Snapshot> {
    tokio::task::spawn_blocking(move || snapshot_sync(&path, at_end)).await?
}

fn snapshot_sync(path: &Path, at_end: bool) -> Result<Snapshot> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let identity = file_identity(&file)?;
    let length = file.metadata()?.len();
    let mut prefix = vec![0; 256];
    let n = file.read(&mut prefix)?;
    prefix.truncate(n);
    file.seek(SeekFrom::Start(if at_end { length } else { 0 }))?;
    Ok((file, identity, length, prefix))
}

async fn boundary_matches(file: &std::fs::File, position: u64, expected: &[u8]) -> Result<bool> {
    if expected.is_empty() {
        return Ok(true);
    }
    let file = file.try_clone()?;
    let expected = expected.to_owned();
    tokio::task::spawn_blocking(move || Ok(read_boundary(&file, position)? == expected)).await?
}

fn read_boundary(file: &std::fs::File, position: u64) -> Result<Vec<u8>> {
    // Windows seek_read changes the shared cursor, including on try_clone handles.
    // This caller exclusively owns the reader while taking its checkpoint.
    #[cfg(windows)]
    {
        use std::io::{Seek, SeekFrom};
        let mut handle = file;
        let original = handle.stream_position()?;
        let result = read_boundary_bytes(file, position);
        handle.seek(SeekFrom::Start(original))?;
        return result;
    }
    #[cfg(not(windows))]
    read_boundary_bytes(file, position)
}

fn read_boundary_bytes(file: &std::fs::File, position: u64) -> Result<Vec<u8>> {
    let length = position.min(256) as usize;
    let mut bytes = vec![0; length];
    let start = position.saturating_sub(length as u64);
    let mut read = 0;
    while read < length {
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::FileExt;
            file.read_at(&mut bytes[read..], start + read as u64)
        };
        #[cfg(windows)]
        let result = {
            use std::os::windows::fs::FileExt;
            file.seek_read(&mut bytes[read..], start + read as u64)
        };
        match result {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bytes.truncate(read);
    Ok(bytes)
}

#[cfg(test)]
#[path = "tail_reader_tests.rs"]
mod tests;
