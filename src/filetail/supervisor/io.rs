use std::io::ErrorKind;

use anyhow::Result;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt};

use super::super::models::FileTailSource;
use super::super::path_policy::{validate_file_tail_path, validate_opened_file_tail_path};
use super::super::platform::{metadata_identity, open_read_no_follow};

const FILE_TAIL_FINGERPRINT_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let (dev, ino) = metadata_identity(metadata);
        Self { dev, ino }
    }
}

#[derive(Debug)]
pub(crate) struct OpenedTailFile {
    pub(crate) file: tokio::fs::File,
    pub(crate) identity: FileIdentity,
    pub(crate) position: u64,
    pub(crate) fingerprint: Vec<u8>,
}

pub(crate) struct BoundedLine {
    pub(crate) bytes_read: usize,
    pub(crate) truncated: bool,
    pub(crate) complete: bool,
}

pub(crate) async fn open_tail_file(
    source: &FileTailSource,
    first_open: bool,
) -> Result<OpenedTailFile> {
    let mut file = open_validated_tail_file(&source.path).await?;
    let metadata = file.metadata().await?;
    let identity = FileIdentity::from_metadata(&metadata);
    let fingerprint = file_prefix_fingerprint(&mut file, FILE_TAIL_FINGERPRINT_BYTES).await?;
    let checkpoint_matches = source.checkpoint_dev == Some(identity.dev)
        && source.checkpoint_ino == Some(identity.ino)
        && source
            .checkpoint_offset
            .is_some_and(|offset| offset <= metadata.len());
    let has_checkpoint = source.checkpoint_dev.is_some()
        || source.checkpoint_ino.is_some()
        || source.checkpoint_offset.is_some();
    let position = if checkpoint_matches {
        source.checkpoint_offset.unwrap_or(0)
    } else if has_checkpoint {
        0
    } else if first_open && source.start_at_end {
        metadata.len()
    } else {
        0
    };
    file.seek(std::io::SeekFrom::Start(position)).await?;
    Ok(OpenedTailFile {
        file,
        identity,
        position,
        fingerprint,
    })
}

pub(crate) async fn reopen_if_rotated_or_truncated(
    source: &FileTailSource,
    identity: FileIdentity,
    position: u64,
    fingerprint: &[u8],
) -> Result<Option<OpenedTailFile>> {
    let metadata = match tokio::fs::metadata(&source.path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            anyhow::bail!("file-tail source disappeared: {}", source.path);
        }
        Err(err) => return Err(err.into()),
    };
    let current = FileIdentity::from_metadata(&metadata);
    if current != identity || metadata.len() < position {
        return reopen_from_start(source).await.map(Some);
    }
    if position > 0 {
        let mut file = open_validated_tail_file(&source.path).await?;
        // Compare only the baseline bytes. Reading farther after a short file grows
        // would misclassify an ordinary append as same-inode replacement, but keep
        // the full current prefix so a real replacement gets a fresh baseline.
        let current_fingerprint =
            file_prefix_fingerprint(&mut file, FILE_TAIL_FINGERPRINT_BYTES).await?;
        if !current_fingerprint.starts_with(fingerprint) {
            let metadata = file.metadata().await?;
            file.seek(std::io::SeekFrom::Start(0)).await?;
            return Ok(Some(OpenedTailFile {
                file,
                identity: FileIdentity::from_metadata(&metadata),
                position: 0,
                fingerprint: current_fingerprint,
            }));
        }
    }
    Ok(None)
}

pub(crate) async fn path_identity_changed(
    source: &FileTailSource,
    identity: FileIdentity,
) -> Result<bool> {
    let metadata = match tokio::fs::metadata(&source.path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            anyhow::bail!("file-tail source disappeared: {}", source.path);
        }
        Err(err) => return Err(err.into()),
    };
    Ok(FileIdentity::from_metadata(&metadata) != identity)
}

async fn reopen_from_start(source: &FileTailSource) -> Result<OpenedTailFile> {
    let mut file = open_validated_tail_file(&source.path).await?;
    let metadata = file.metadata().await?;
    let fingerprint = file_prefix_fingerprint(&mut file, FILE_TAIL_FINGERPRINT_BYTES).await?;
    file.seek(std::io::SeekFrom::Start(0)).await?;
    Ok(OpenedTailFile {
        file,
        identity: FileIdentity::from_metadata(&metadata),
        position: 0,
        fingerprint,
    })
}

async fn file_prefix_fingerprint(
    file: &mut tokio::fs::File,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0; limit.min(FILE_TAIL_FINGERPRINT_BYTES)];
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let n = file.read(&mut buf).await?;
    buf.truncate(n);
    file.seek(std::io::SeekFrom::Start(0)).await?;
    Ok(buf)
}

async fn open_validated_tail_file(path: &str) -> Result<tokio::fs::File> {
    validate_file_tail_path(path)?;
    let path = path.to_string();
    let std_file = tokio::task::spawn_blocking({
        let path = path.clone();
        move || open_read_no_follow(std::path::Path::new(&path))
    })
    .await??;
    let metadata = std_file.metadata()?;
    validate_opened_file_tail_path(&path, &metadata)?;
    Ok(tokio::fs::File::from_std(std_file))
}

pub(crate) fn open_validated_tail_file_sync(path: &str) -> Result<std::fs::File> {
    validate_file_tail_path(path)?;
    let file = open_read_no_follow(std::path::Path::new(path))?;
    let metadata = file.metadata()?;
    validate_opened_file_tail_path(path, &metadata)?;
    Ok(file)
}

pub(crate) async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut Vec<u8>,
    max_line_bytes: usize,
) -> std::io::Result<BoundedLine> {
    let mut bytes_read = 0;
    let mut truncated = false;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(BoundedLine {
                bytes_read,
                truncated,
                complete: false,
            });
        }

        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let consume_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        let remaining = max_line_bytes.saturating_sub(out.len());
        let copy_len = remaining.min(consume_len);
        out.extend_from_slice(&available[..copy_len]);
        if copy_len < consume_len {
            truncated = true;
        }
        reader.consume(consume_len);
        bytes_read += consume_len;

        if newline_pos.is_some() {
            return Ok(BoundedLine {
                bytes_read,
                truncated,
                complete: true,
            });
        }
    }
}
