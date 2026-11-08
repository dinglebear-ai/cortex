//! A bounded durable outbox and source cursor, committed by atomic replacement.
use super::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_STATE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct State {
    pub cursor: super::super::tail_reader::TailCursor,
    pub pending: Option<AgentFileTailRecord>,
}

struct DiskStore {
    path: PathBuf,
    _lock: File,
    last_bytes: Vec<u8>,
}

pub(super) struct Store(std::sync::Arc<std::sync::Mutex<DiskStore>>);

impl Store {
    pub(super) async fn open(path: &Path) -> Result<(Self, Option<State>)> {
        let path = path.to_owned();
        let (store, state) = tokio::task::spawn_blocking(move || DiskStore::open(&path)).await??;
        Ok((
            Self(std::sync::Arc::new(std::sync::Mutex::new(store))),
            state,
        ))
    }

    pub(super) async fn save(&self, state: &State) -> Result<()> {
        let owner = self.0.clone();
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            owner
                .lock()
                .map_err(|_| anyhow::anyhow!("file-tail state owner poisoned"))?
                .save(&state)
        })
        .await?
    }
}

pub fn checkpoint_path(base: &Path, config: &FileTailForwardConfig) -> PathBuf {
    let key = Sha256::digest(format!(
        "{}\0{}\0{}\0{}",
        config.hostname,
        config.target.trim_end_matches('/'),
        config.source.path.display(),
        config.source.tag.as_deref().unwrap_or("file-tail")
    ));
    base.with_file_name(format!("file-tail-{}.json", hex::encode(key)))
}

impl DiskStore {
    pub(super) fn open(path: &Path) -> Result<(Self, Option<State>)> {
        let parent = path
            .parent()
            .context("file-tail checkpoint has no parent")?;
        fs::create_dir_all(parent)?;
        let lock = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock()
            .context("another file-tail forwarder owns this checkpoint")?;
        let bytes = match private_options().read(true).open(path) {
            Ok(file) => {
                anyhow::ensure!(
                    file.metadata()?.len() <= MAX_STATE_BYTES,
                    "file-tail checkpoint exceeds bounded state budget"
                );
                let mut bytes = Vec::new();
                file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes)?;
                anyhow::ensure!(
                    bytes.len() as u64 <= MAX_STATE_BYTES,
                    "file-tail checkpoint grew beyond state budget"
                );
                bytes
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        let state = if bytes.is_empty() {
            // An existing empty file is corruption, not a new installation.
            anyhow::ensure!(
                !path.exists(),
                "empty file-tail checkpoint; operator recovery required"
            );
            None
        } else {
            Some(
                serde_json::from_slice(&bytes)
                    .context("invalid file-tail checkpoint; preserving it for recovery")?,
            )
        };
        Ok((
            Self {
                path: path.to_owned(),
                _lock: lock,
                last_bytes: bytes,
            },
            state,
        ))
    }

    pub(super) fn save(&mut self, state: &State) -> Result<()> {
        let bytes = serde_json::to_vec(state)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_STATE_BYTES,
            "file-tail checkpoint exceeds state budget"
        );
        if bytes == self.last_bytes {
            return Ok(());
        }
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce)?;
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", hex::encode(nonce)));
        let result = (|| -> Result<()> {
            let mut file = private_options()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)?;
            #[cfg(unix)]
            File::open(self.path.parent().unwrap())?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.last_bytes = bytes;
        Ok(())
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}
