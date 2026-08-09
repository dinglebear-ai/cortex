use super::{
    GitWatchAction, GitWatchEvent, GitWatchEventKind, GitWatchQueueErrorKind, GitWatchQueueOptions,
    WatchEnqueueResult, git_watch_channel,
};
use crate::git_observer::watcher::{WatchPlan, WatchTarget};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn target(path: &str, repository_keys: &[&str], discovers_repositories: bool) -> WatchTarget {
    WatchTarget {
        path: PathBuf::from(path),
        repository_keys: repository_keys
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        discovers_repositories,
    }
}

fn plan() -> WatchPlan {
    WatchPlan {
        targets: vec![
            target("/workspace", &[], true),
            target("/workspace/a/.git", &["repo-a"], false),
            target("/workspace/a/.git/HEAD", &["repo-a"], false),
            target("/workspace/a/.git/worktrees", &["repo-a"], false),
            target("/workspace/b/.git", &["repo-b"], false),
            target("/workspace/b/.git/HEAD", &["repo-b"], false),
        ],
    }
}

fn options(channel_capacity: usize, max_pending_repositories: usize) -> GitWatchQueueOptions {
    GitWatchQueueOptions {
        channel_capacity,
        max_pending_repositories,
        debounce: Duration::from_millis(500),
    }
}

fn event(kind: GitWatchEventKind, path: &str, observed_at: Instant) -> GitWatchEvent {
    GitWatchEvent {
        kind,
        paths: vec![PathBuf::from(path)],
        observed_at,
    }
}

#[test]
fn burst_events_coalesce_to_one_repository_at_the_last_event_deadline() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(128, 16)).unwrap();
    for offset in 0..100 {
        assert_eq!(
            sender.try_send(event(
                GitWatchEventKind::Change,
                "/workspace/a/.git/HEAD",
                start + Duration::from_millis(offset),
            )),
            WatchEnqueueResult::Queued
        );
    }

    assert!(queue.poll(start + Duration::from_millis(598)).is_empty());
    assert_eq!(
        queue.poll(start + Duration::from_millis(599)),
        vec![GitWatchAction::ReconcileRepository {
            repository_key: "repo-a".to_string(),
        }]
    );
    assert!(queue.poll(start + Duration::from_secs(2)).is_empty());
}

#[test]
fn ready_repositories_are_returned_once_in_deterministic_key_order() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(8, 8)).unwrap();
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/workspace/b/.git/HEAD",
            start,
        )),
        WatchEnqueueResult::Queued
    );
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Remove,
            "/workspace/a/.git/index.lock",
            start + Duration::from_millis(1),
        )),
        WatchEnqueueResult::Queued
    );

    assert_eq!(
        queue.poll(start + Duration::from_millis(501)),
        vec![
            GitWatchAction::ReconcileRepository {
                repository_key: "repo-a".to_string(),
            },
            GitWatchAction::ReconcileRepository {
                repository_key: "repo-b".to_string(),
            },
        ]
    );
}

#[test]
fn project_root_create_schedules_discovery_without_source_tree_reconcile() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(8, 8)).unwrap();
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Create,
            "/workspace/new-repository",
            start,
        )),
        WatchEnqueueResult::Queued
    );
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/workspace/random-source.rs",
            start + Duration::from_millis(10),
        )),
        WatchEnqueueResult::Queued
    );

    assert_eq!(
        queue.poll(start + Duration::from_millis(500)),
        vec![GitWatchAction::DiscoverRepositories {
            project_root: PathBuf::from("/workspace"),
        }]
    );
}

#[test]
fn linked_control_directory_create_routes_to_repository_not_root_discovery() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(8, 8)).unwrap();
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Create,
            "/workspace/a/.git/worktrees/new-linked/HEAD",
            start,
        )),
        WatchEnqueueResult::Queued
    );

    assert_eq!(
        queue.poll(start + Duration::from_millis(500)),
        vec![GitWatchAction::ReconcileRepository {
            repository_key: "repo-a".to_string(),
        }]
    );
}

#[test]
fn bounded_channel_overflow_signals_one_full_reconcile_and_drops_pending_work() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(1, 8)).unwrap();
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/workspace/a/.git/HEAD",
            start,
        )),
        WatchEnqueueResult::Queued
    );
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/workspace/b/.git/HEAD",
            start,
        )),
        WatchEnqueueResult::OverflowSignaled
    );

    assert_eq!(queue.poll(start), vec![GitWatchAction::FullReconcile]);
    assert!(queue.poll(start + Duration::from_secs(2)).is_empty());
}

#[test]
fn pending_repository_limit_escalates_to_full_reconcile() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(8, 1)).unwrap();
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/workspace/a/.git/HEAD",
            start,
        )),
        WatchEnqueueResult::Queued
    );
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/workspace/b/.git/HEAD",
            start,
        )),
        WatchEnqueueResult::Queued
    );

    assert_eq!(queue.poll(start), vec![GitWatchAction::FullReconcile]);
    assert_eq!(queue.pending_repository_count(), 0);
}

#[test]
fn explicit_rescan_event_and_unrelated_paths_are_handled_safely() {
    let start = Instant::now();
    let (sender, mut queue) = git_watch_channel(&plan(), options(8, 8)).unwrap();
    assert_eq!(
        sender.try_send(event(
            GitWatchEventKind::Change,
            "/outside/unrelated",
            start,
        )),
        WatchEnqueueResult::Queued
    );
    assert!(queue.poll(start + Duration::from_secs(1)).is_empty());

    assert_eq!(
        sender.try_send(GitWatchEvent {
            kind: GitWatchEventKind::Rescan,
            paths: Vec::new(),
            observed_at: start + Duration::from_secs(2),
        }),
        WatchEnqueueResult::Queued
    );
    assert_eq!(
        queue.poll(start + Duration::from_secs(2)),
        vec![GitWatchAction::FullReconcile]
    );
}

#[test]
fn invalid_queue_options_fail_before_channel_creation() {
    let error = git_watch_channel(&plan(), options(0, 8)).unwrap_err();
    assert_eq!(error.kind, GitWatchQueueErrorKind::InvalidChannelCapacity);

    let error = git_watch_channel(&plan(), options(8, 0)).unwrap_err();
    assert_eq!(
        error.kind,
        GitWatchQueueErrorKind::InvalidPendingRepositoryLimit
    );

    let error = git_watch_channel(
        &plan(),
        GitWatchQueueOptions {
            channel_capacity: 8,
            max_pending_repositories: 8,
            debounce: Duration::ZERO,
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, GitWatchQueueErrorKind::InvalidDebounce);
}
