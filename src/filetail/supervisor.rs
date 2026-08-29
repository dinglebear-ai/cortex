use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tokio::io::BufReader;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::ingest::IngestTx;

use super::models::{FileTailSource, FileTailStatus};
use super::platform::metadata_identity;
use super::registry::FileTailRegistry;

mod io;
mod line_entry;

pub(crate) use self::io::{
    FileIdentity, open_tail_file, open_validated_tail_file_sync, path_identity_changed,
    read_bounded_line, reopen_if_rotated_or_truncated,
};
pub(crate) use self::line_entry::file_tail_line_to_entry;
#[cfg(test)]
pub(crate) use self::line_entry::tail_file_once_for_test;

const FILE_TAIL_ROTATION_GRACE: Duration = Duration::from_millis(1000);
const CHECKPOINT_FLUSH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub(crate) struct FileTailSupervisor {
    registry: Arc<FileTailRegistry>,
    ingest: IngestTx,
    token: CancellationToken,
    tasks: Arc<Mutex<HashMap<String, TailTask>>>,
    max_line_bytes: usize,
}

struct TailTask {
    handle: JoinHandle<()>,
    status: Arc<Mutex<FileTailStatus>>,
    source: FileTailSource,
}

pub(crate) struct FileTailShutdown {
    #[cfg(test)]
    pub(crate) statuses: Vec<FileTailStatus>,
}

impl FileTailSupervisor {
    pub(crate) fn new(
        registry: Arc<FileTailRegistry>,
        ingest: IngestTx,
        token: CancellationToken,
        max_line_bytes: usize,
    ) -> Self {
        Self {
            registry,
            ingest,
            token,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            max_line_bytes,
        }
    }

    pub(crate) fn statuses(&self) -> Vec<FileTailStatus> {
        let mut out: Vec<_> = self
            .tasks
            .lock()
            .values()
            .map(|task| task.status.lock().clone())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub(crate) async fn shutdown(&self, timeout: Duration) -> FileTailShutdown {
        self.token.cancel();
        let tasks: Vec<_> = self.tasks.lock().drain().map(|(_, task)| task).collect();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut stopped = Vec::new();
        for mut task in tasks {
            let source_id = task.source.id.clone();
            let abort_handle = task.handle.abort_handle();
            let outcome = tokio::time::timeout_at(deadline, &mut task.handle).await;
            let failure = match outcome {
                Ok(Ok(())) => None,
                Ok(Err(err)) if err.is_panic() => {
                    Some(format!("file-tail task panicked during shutdown: {err}"))
                }
                Ok(Err(err)) if err.is_cancelled() => Some(format!(
                    "file-tail task was unexpectedly cancelled during shutdown: {err}"
                )),
                Ok(Err(err)) => Some(format!("file-tail task join failed during shutdown: {err}")),
                Err(_) => {
                    abort_handle.abort();
                    let _ = task.handle.await;
                    Some(format!(
                        "file-tail task shutdown timed out after {timeout:?}"
                    ))
                }
            };
            let mut status = task.status.lock();
            status.running = false;
            if let Some(failure) = failure {
                tracing::error!(source_id = %source_id, error = %failure, "file-tail shutdown failed");
                append_status_error(&mut status, failure);
            } else if let Some(error) = status.last_error.as_deref() {
                tracing::error!(source_id = %source_id, error, "file-tail task stopped with an error");
            }
            stopped.push(status.clone());
        }
        stopped.sort_by(|a, b| a.id.cmp(&b.id));
        FileTailShutdown {
            #[cfg(test)]
            statuses: stopped,
        }
    }

    pub(crate) fn reconcile(&self) -> Result<()> {
        let sources = self.registry.list()?;
        let enabled: HashMap<String, FileTailSource> = sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| (source.id.clone(), source.clone()))
            .collect();

        let mut tasks = self.tasks.lock();
        tasks.retain(|id, task| {
            let keep_running = enabled
                .get(id)
                .is_some_and(|source| source.same_definition(&task.source));
            if !keep_running {
                task.status.lock().running = false;
                task.handle.abort();
            }
            keep_running
        });
        for source in sources {
            if source.enabled && !tasks.contains_key(&source.id) {
                let source = self.ensure_initial_checkpoint(source)?;
                let (id, task) = self.build_task(source);
                tasks.insert(id, task);
            }
        }
        Ok(())
    }

    fn ensure_initial_checkpoint(&self, mut source: FileTailSource) -> Result<FileTailSource> {
        let has_checkpoint = source.checkpoint_dev.is_some()
            || source.checkpoint_ino.is_some()
            || source.checkpoint_offset.is_some();
        if has_checkpoint {
            return Ok(source);
        }

        let file = open_validated_tail_file_sync(&source.path)?;
        let metadata = file.metadata()?;
        let offset = if source.start_at_end {
            metadata.len()
        } else {
            0
        };
        let (dev, ino) = metadata_identity(&metadata);
        source.checkpoint_dev = Some(dev);
        source.checkpoint_ino = Some(ino);
        source.checkpoint_offset = Some(offset);
        self.registry
            .update_checkpoint(&source.id, dev, ino, offset, &now_iso())?;
        Ok(source)
    }

    fn build_task(&self, source: FileTailSource) -> (String, TailTask) {
        let id = source.id.clone();
        let initial_source = source.clone();
        let task_source = source;
        let status = Arc::new(Mutex::new(FileTailStatus {
            id: id.clone(),
            running: true,
            last_line_at: None,
            last_read_at: None,
            last_checkpoint_at: None,
            blocked_on_writer_since: None,
            last_error: None,
        }));
        let task_status = Arc::clone(&status);
        let ingest = self.ingest.clone();
        let token = self.token.clone();
        let registry = Arc::clone(&self.registry);
        let max_line_bytes = self.max_line_bytes;
        let handle = tokio::spawn(async move {
            tail_file_loop(
                initial_source,
                registry,
                ingest,
                token,
                task_status,
                max_line_bytes,
            )
            .await;
        });
        (
            id,
            TailTask {
                handle,
                status,
                source: task_source,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn running_source_for_test(&self, id: &str) -> Option<FileTailSource> {
        self.tasks.lock().get(id).map(|task| task.source.clone())
    }
}

async fn tail_file_loop(
    initial_source: FileTailSource,
    registry: Arc<FileTailRegistry>,
    ingest: IngestTx,
    token: CancellationToken,
    status: Arc<Mutex<FileTailStatus>>,
    max_line_bytes: usize,
) {
    let source_id = initial_source.id.clone();
    let mut initial_source = Some(initial_source);
    loop {
        if token.is_cancelled() {
            status.lock().running = false;
            return;
        }
        let live_source = match registry.get(&source_id) {
            Ok(Some(source)) if source.enabled => source,
            Ok(_) => {
                status.lock().running = false;
                return;
            }
            Err(err) => {
                tracing::error!(
                    source_id = %source_id,
                    error = %err,
                    "file-tail source reload failed; retrying"
                );
                status.lock().last_error = Some(err.to_string());
                tokio::select! {
                    _ = token.cancelled() => {
                        status.lock().running = false;
                        return;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                continue;
            }
        };
        let source = match initial_source.take() {
            Some(source) if source.same_definition(&live_source) => source,
            _ => live_source,
        };
        match tail_file_until_cancelled(
            &source,
            Arc::clone(&registry),
            ingest.clone(),
            token.clone(),
            Arc::clone(&status),
            max_line_bytes,
        )
        .await
        {
            Ok(()) => {
                status.lock().running = false;
                return;
            }
            Err(err) => {
                tracing::error!(
                    source_id = %source.id,
                    path = %source.path,
                    error = %err,
                    "file-tail source failed; retrying"
                );
                status.lock().last_error = Some(err.to_string());
                tokio::select! {
                    _ = token.cancelled() => {
                        status.lock().running = false;
                        return;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }
        }
    }
}

async fn tail_file_until_cancelled(
    source: &FileTailSource,
    registry: Arc<FileTailRegistry>,
    ingest: IngestTx,
    token: CancellationToken,
    status: Arc<Mutex<FileTailStatus>>,
    max_line_bytes: usize,
) -> Result<()> {
    let opened = open_tail_file(source, true)
        .await
        .with_context(|| format!("open {}", source.path))?;
    let mut reader = BufReader::new(opened.file);
    let mut position = opened.position;
    // `position` includes bytes buffered from an unterminated line. Only the
    // durable position is safe to checkpoint and resume from.
    let mut durable_position = opened.position;
    let mut identity = opened.identity;
    let mut fingerprint = opened.fingerprint;
    let mut line = Vec::new();
    let mut pending_rotation_since: Option<Instant> = None;
    let mut last_checkpoint_flush = Instant::now();
    let mut checkpoint_dirty = false;
    let mut checkpoint_time = now_iso();
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                if checkpoint_dirty {
                    persist_checkpoint(
                        Arc::clone(&registry), source.id.clone(), identity, durable_position,
                        checkpoint_time.clone(),
                    ).await?;
                    status.lock().last_checkpoint_at = Some(checkpoint_time.clone());
                }
                return Ok(())
            },
            read = read_bounded_line(&mut reader, &mut line, max_line_bytes) => {
                let read = read?;
                if read.bytes_read == 0 {
                    if checkpoint_dirty && last_checkpoint_flush.elapsed() >= CHECKPOINT_FLUSH_INTERVAL {
                        persist_checkpoint(
                            Arc::clone(&registry), source.id.clone(), identity, durable_position,
                            checkpoint_time.clone(),
                        ).await?;
                        checkpoint_dirty = false;
                        last_checkpoint_flush = Instant::now();
                        status.lock().last_checkpoint_at = Some(checkpoint_time.clone());
                    }
                    if path_identity_changed(source, identity).await? {
                        let since = pending_rotation_since.get_or_insert_with(Instant::now);
                        if since.elapsed() < FILE_TAIL_ROTATION_GRACE {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                    } else {
                        pending_rotation_since = None;
                    }
                    if let Some(next) = reopen_if_rotated_or_truncated(source, identity, position, &fingerprint).await? {
                        if !line.is_empty() {
                            let now = now_iso();
                            let partial = PartialLineBeforeReopen {
                                source,
                                registry: &registry,
                                ingest: &ingest,
                                status: &status,
                                line: &line,
                                identity,
                                position,
                                now: &now,
                            };
                            ingest_partial_line_before_reopen(partial).await?;
                        }
                        reader = BufReader::new(next.file);
                        position = next.position;
                        durable_position = next.position;
                        identity = next.identity;
                        fingerprint = next.fingerprint;
                        pending_rotation_since = None;
                        line.clear();
                    } else {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    continue;
                }
                position = position.saturating_add(read.bytes_read as u64);
                pending_rotation_since = None;
                if !read.complete {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                let msg = String::from_utf8_lossy(&line);
                let msg = msg.trim_end_matches(['\r', '\n']);
                if msg.is_empty() {
                    durable_position = position;
                    checkpoint_time = now_iso();
                    checkpoint_dirty = true;
                    line.clear();
                    continue;
                }
                let now = now_iso();
                let entry = file_tail_line_to_entry(source, msg, &now);
                {
                    let mut status = status.lock();
                    status.last_read_at = Some(now.clone());
                    status.blocked_on_writer_since = Some(now.clone());
                }
                ingest.send_durable(entry).await?;
                durable_position = position;
                checkpoint_time.clone_from(&now);
                checkpoint_dirty = true;
                let checkpoint_persisted = if last_checkpoint_flush.elapsed() >= CHECKPOINT_FLUSH_INTERVAL {
                    persist_checkpoint(
                        Arc::clone(&registry), source.id.clone(), identity, durable_position, now.clone(),
                    ).await?;
                    checkpoint_dirty = false;
                    last_checkpoint_flush = Instant::now();
                    true
                } else {
                    false
                };
                line.clear();
                let mut status = status.lock();
                status.last_line_at = Some(now);
                if checkpoint_persisted {
                    status.last_checkpoint_at = status.last_line_at.clone();
                }
                status.blocked_on_writer_since = None;
                status.last_error = if read.truncated {
                    Some(format!(
                        "truncated oversized line from {} to {max_line_bytes} bytes",
                        source.path
                    ))
                } else {
                    None
                };
            }
        }
    }
}

async fn persist_checkpoint(
    registry: Arc<FileTailRegistry>,
    source_id: String,
    identity: FileIdentity,
    position: u64,
    now: String,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        registry.update_checkpoint(&source_id, identity.dev, identity.ino, position, &now)
    })
    .await
    .context("join file-tail checkpoint persistence")??;
    Ok(())
}

fn append_status_error(status: &mut FileTailStatus, error: String) {
    status.last_error = Some(match status.last_error.take() {
        Some(previous) => format!("{previous}; {error}"),
        None => error,
    });
}

struct PartialLineBeforeReopen<'a> {
    source: &'a FileTailSource,
    registry: &'a FileTailRegistry,
    ingest: &'a IngestTx,
    status: &'a Mutex<FileTailStatus>,
    line: &'a [u8],
    identity: FileIdentity,
    position: u64,
    now: &'a str,
}

async fn ingest_partial_line_before_reopen(partial: PartialLineBeforeReopen<'_>) -> Result<()> {
    let msg = String::from_utf8_lossy(partial.line);
    let msg = msg.trim_end_matches(['\r', '\n']);
    if msg.is_empty() {
        return Ok(());
    }
    partial
        .ingest
        .send_durable(file_tail_line_to_entry(partial.source, msg, partial.now))
        .await?;
    partial.registry.update_checkpoint(
        &partial.source.id,
        partial.identity.dev,
        partial.identity.ino,
        partial.position,
        partial.now,
    )?;
    let mut status = partial.status.lock();
    status.last_line_at = Some(partial.now.to_string());
    status.last_checkpoint_at = Some(partial.now.to_string());
    status.last_error = Some(format!(
        "ingested unterminated partial line before rotation/truncation for {}",
        partial.source.path
    ));
    Ok(())
}
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
