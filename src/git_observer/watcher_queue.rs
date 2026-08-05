//! Debounced bounded Git watcher queue.

use super::{WatchPlan, WatchTarget};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitWatchQueueOptions {
    pub channel_capacity: usize,
    pub max_pending_repositories: usize,
    pub debounce: Duration,
}

impl Default for GitWatchQueueOptions {
    fn default() -> Self {
        Self {
            channel_capacity: 1_024,
            max_pending_repositories: 1_024,
            debounce: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitWatchEventKind {
    Change,
    Create,
    Remove,
    Rescan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWatchEvent {
    pub kind: GitWatchEventKind,
    pub paths: Vec<PathBuf>,
    pub observed_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitWatchAction {
    ReconcileRepository { repository_key: String },
    DiscoverRepositories { project_root: PathBuf },
    FullReconcile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEnqueueResult {
    Queued,
    OverflowSignaled,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitWatchQueueErrorKind {
    InvalidChannelCapacity,
    InvalidPendingRepositoryLimit,
    InvalidDebounce,
    NonCanonicalTargetPath { target_index: usize },
    EmptyRepositoryKey { target_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWatchQueueError {
    pub kind: GitWatchQueueErrorKind,
}

impl fmt::Display for GitWatchQueueErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidChannelCapacity => {
                formatter.write_str("channel_capacity must be positive")
            }
            Self::InvalidPendingRepositoryLimit => {
                formatter.write_str("max_pending_repositories must be positive")
            }
            Self::InvalidDebounce => formatter.write_str("debounce must be positive"),
            Self::NonCanonicalTargetPath { target_index } => {
                write!(
                    formatter,
                    "watch target {target_index} path is not canonical"
                )
            }
            Self::EmptyRepositoryKey { target_index } => {
                write!(
                    formatter,
                    "watch target {target_index} has an empty repository key"
                )
            }
        }
    }
}

impl fmt::Display for GitWatchQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Git watch queue: {}", self.kind)
    }
}

impl std::error::Error for GitWatchQueueError {}

#[derive(Debug, Clone)]
pub struct GitWatchSender {
    sender: mpsc::Sender<GitWatchEvent>,
    overflow: Arc<AtomicBool>,
}

impl GitWatchSender {
    pub fn try_send(&self, event: GitWatchEvent) -> WatchEnqueueResult {
        match self.sender.try_send(event) {
            Ok(()) => WatchEnqueueResult::Queued,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflow.store(true, Ordering::Release);
                WatchEnqueueResult::OverflowSignaled
            }
            Err(mpsc::error::TrySendError::Closed(_)) => WatchEnqueueResult::Closed,
        }
    }

    pub fn signal_overflow(&self) {
        self.overflow.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub struct GitWatchQueue {
    receiver: mpsc::Receiver<GitWatchEvent>,
    overflow: Arc<AtomicBool>,
    targets: Vec<WatchTarget>,
    pending_repositories: BTreeMap<String, Instant>,
    pending_discoveries: BTreeMap<PathBuf, Instant>,
    options: GitWatchQueueOptions,
}

fn is_canonical_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

fn validate(plan: &WatchPlan, options: GitWatchQueueOptions) -> Result<(), GitWatchQueueError> {
    let invalid = |kind| Err(GitWatchQueueError { kind });
    if options.channel_capacity == 0 {
        return invalid(GitWatchQueueErrorKind::InvalidChannelCapacity);
    }
    if options.max_pending_repositories == 0 {
        return invalid(GitWatchQueueErrorKind::InvalidPendingRepositoryLimit);
    }
    if options.debounce.is_zero() {
        return invalid(GitWatchQueueErrorKind::InvalidDebounce);
    }
    for (target_index, target) in plan.targets.iter().enumerate() {
        if !is_canonical_absolute(&target.path) {
            return invalid(GitWatchQueueErrorKind::NonCanonicalTargetPath { target_index });
        }
        if target
            .repository_keys
            .iter()
            .any(|key| key.trim().is_empty())
        {
            return invalid(GitWatchQueueErrorKind::EmptyRepositoryKey { target_index });
        }
    }
    Ok(())
}

pub fn git_watch_channel(
    plan: &WatchPlan,
    options: GitWatchQueueOptions,
) -> Result<(GitWatchSender, GitWatchQueue), GitWatchQueueError> {
    validate(plan, options)?;
    let (sender, receiver) = mpsc::channel(options.channel_capacity);
    let overflow = Arc::new(AtomicBool::new(false));
    let mut targets = plan.targets.clone();
    targets.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.repository_keys.cmp(&right.repository_keys))
            .then_with(|| {
                left.discovers_repositories
                    .cmp(&right.discovers_repositories)
            })
    });
    Ok((
        GitWatchSender {
            sender,
            overflow: Arc::clone(&overflow),
        },
        GitWatchQueue {
            receiver,
            overflow,
            targets,
            pending_repositories: BTreeMap::new(),
            pending_discoveries: BTreeMap::new(),
            options,
        },
    ))
}

impl GitWatchQueue {
    fn signal_overflow(&mut self) {
        self.overflow.store(true, Ordering::Release);
    }

    fn clear_pending(&mut self) {
        self.pending_repositories.clear();
        self.pending_discoveries.clear();
    }

    fn discard_buffered(&mut self) {
        while self.receiver.try_recv().is_ok() {}
    }

    fn pending_item_count(&self) -> usize {
        self.pending_repositories.len() + self.pending_discoveries.len()
    }

    fn push_repository(&mut self, repository_key: &str, observed_at: Instant) {
        if let Some(last_seen) = self.pending_repositories.get_mut(repository_key) {
            *last_seen = observed_at;
            return;
        }
        if self.pending_item_count() >= self.options.max_pending_repositories {
            self.signal_overflow();
            return;
        }
        self.pending_repositories
            .insert(repository_key.to_string(), observed_at);
    }

    fn push_discovery(&mut self, project_root: &Path, observed_at: Instant) {
        if let Some(last_seen) = self.pending_discoveries.get_mut(project_root) {
            *last_seen = observed_at;
            return;
        }
        if self.pending_item_count() >= self.options.max_pending_repositories {
            self.signal_overflow();
            return;
        }
        self.pending_discoveries
            .insert(project_root.to_path_buf(), observed_at);
    }

    fn matching_targets(&self, path: &Path) -> Vec<WatchTarget> {
        let max_depth = self
            .targets
            .iter()
            .filter(|target| path == target.path || path.starts_with(&target.path))
            .map(|target| target.path.components().count())
            .max();
        let Some(max_depth) = max_depth else {
            return Vec::new();
        };
        self.targets
            .iter()
            .filter(|target| {
                (path == target.path || path.starts_with(&target.path))
                    && target.path.components().count() == max_depth
            })
            .cloned()
            .collect()
    }

    fn process_event(&mut self, event: GitWatchEvent) {
        if event.kind == GitWatchEventKind::Rescan {
            self.signal_overflow();
            return;
        }
        for path in event.paths {
            let targets = self.matching_targets(&path);
            if targets.is_empty() {
                continue;
            }
            let mut repositories = BTreeSet::new();
            let mut discoveries = BTreeSet::new();
            for target in targets {
                repositories.extend(target.repository_keys);
                if event.kind == GitWatchEventKind::Create && target.discovers_repositories {
                    discoveries.insert(target.path);
                }
            }
            for repository_key in repositories {
                self.push_repository(&repository_key, event.observed_at);
            }
            for project_root in discoveries {
                self.push_discovery(&project_root, event.observed_at);
            }
            if self.overflow.load(Ordering::Acquire) {
                return;
            }
        }
    }

    fn overflow_action(&mut self) -> Option<Vec<GitWatchAction>> {
        if !self.overflow.swap(false, Ordering::AcqRel) {
            return None;
        }
        self.clear_pending();
        self.discard_buffered();
        Some(vec![GitWatchAction::FullReconcile])
    }

    fn is_ready(&self, now: Instant, last_seen: Instant) -> bool {
        now.checked_duration_since(last_seen)
            .is_some_and(|elapsed| elapsed >= self.options.debounce)
    }

    pub fn poll(&mut self, now: Instant) -> Vec<GitWatchAction> {
        if let Some(actions) = self.overflow_action() {
            return actions;
        }
        while let Ok(event) = self.receiver.try_recv() {
            self.process_event(event);
            if self.overflow.load(Ordering::Acquire) {
                break;
            }
        }
        if let Some(actions) = self.overflow_action() {
            return actions;
        }

        let ready_repositories = self
            .pending_repositories
            .iter()
            .filter(|(_, last_seen)| self.is_ready(now, **last_seen))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let ready_discoveries = self
            .pending_discoveries
            .iter()
            .filter(|(_, last_seen)| self.is_ready(now, **last_seen))
            .map(|(root, _)| root.clone())
            .collect::<Vec<_>>();

        let mut actions = Vec::with_capacity(ready_repositories.len() + ready_discoveries.len());
        for repository_key in ready_repositories {
            self.pending_repositories.remove(&repository_key);
            actions.push(GitWatchAction::ReconcileRepository { repository_key });
        }
        for project_root in ready_discoveries {
            self.pending_discoveries.remove(&project_root);
            actions.push(GitWatchAction::DiscoverRepositories { project_root });
        }
        actions
    }

    pub fn pending_repository_count(&self) -> usize {
        self.pending_repositories.len()
    }
}

#[cfg(test)]
#[path = "watcher_queue_tests.rs"]
mod tests;
