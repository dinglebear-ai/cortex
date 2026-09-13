//! Shared bounded framing and rotation handling for host-local agent files.
use crate::filetail::platform::file_identity;
use crate::filetail::supervisor::read_bounded_line;
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::BufReader;

const MAX_LINE_BYTES: usize = 64 * 1024;

pub(super) struct TailReader {
    path: PathBuf,
    reader: BufReader<tokio::fs::File>,
    identity: (u64, u64),
    position: u64,
    prefix: Vec<u8>,
    partial: Vec<u8>,
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
        })
    }

    /// None means temporary EOF; callers may sleep before polling again.
    pub(super) async fn next_line(&mut self) -> Result<Option<String>> {
        // A copytruncate can regrow beyond our old offset between polls.
        // Check the same inode's prefix before reading a suffix at that offset.
        if self.reader.buffer().is_empty()
            && let Ok((file, identity, length, prefix)) = snapshot(self.path.clone(), false).await
            && identity == self.identity
            && (length < self.position || !prefix.starts_with(&self.prefix))
        {
            self.reader = BufReader::new(tokio::fs::File::from_std(file));
            self.position = 0;
            self.prefix = prefix;
            if !self.partial.is_empty() {
                let line = String::from_utf8_lossy(&self.partial).into_owned();
                self.partial.clear();
                return Ok(Some(line));
            }
        }
        let read = read_bounded_line(&mut self.reader, &mut self.partial, MAX_LINE_BYTES).await?;
        self.position = self.position.saturating_add(read.bytes_read as u64);
        if read.complete {
            let line = String::from_utf8_lossy(&self.partial)
                .trim_end_matches(['\r', '\n'])
                .to_owned();
            self.partial.clear();
            return Ok(Some(line));
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
            self.position = 0;
            self.prefix = prefix;
            // Flush an unterminated old record once, never join it to the new file.
            if !self.partial.is_empty() {
                let line = String::from_utf8_lossy(&self.partial).into_owned();
                self.partial.clear();
                return Ok(Some(line));
            }
        } else if prefix.len() > self.prefix.len() {
            self.prefix = prefix;
        }
        Ok(None)
    }
}

type Snapshot = (std::fs::File, (u64, u64), u64, Vec<u8>);
async fn snapshot(path: PathBuf, at_end: bool) -> Result<Snapshot> {
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path)?;
        let identity = file_identity(&file)?;
        let length = file.metadata()?.len();
        let mut prefix = vec![0; 256];
        let n = file.read(&mut prefix)?;
        prefix.truncate(n);
        file.seek(SeekFrom::Start(if at_end { length } else { 0 }))?;
        Ok((file, identity, length, prefix))
    })
    .await?
}

#[cfg(test)]
#[path = "tail_reader_tests.rs"]
mod tests;
