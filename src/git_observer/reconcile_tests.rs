use super::{
    GitCommandResult, GitCommandRunner, ProcessGitRunner, ReconcileOptions, ReconcileStage,
    ReconcileWarningKind, reconcile_one_repository, reconcile_one_repository_with_runner,
};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    RepositoryObservationKind, get_repository_by_key, list_repository_observations,
    list_repository_worktrees,
};
use crate::db::init_pool;
use crate::git_observer::test_support::{GitFixture, git_available};
use anyhow::{Result, bail};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn options() -> ReconcileOptions {
    ReconcileOptions {
        hostname: "dookie".to_string(),
        command_timeout: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn real_repository_reconcile_persists_topology_and_only_changed_observations() {
    if !git_available() {
        eprintln!("skipping repository reconcile test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(db_dir.path().join("reconcile.db"))).unwrap();

    let first = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T13:00:00.000Z",
    )
    .await
    .unwrap();
    assert!(first.warnings.is_empty());
    assert_eq!(first.inserted_observations.len(), 7);
    let topology = first.topology.as_ref().unwrap();
    assert_eq!(topology.worktrees.len(), 3);
    assert_eq!(
        topology.repository.common_git_dir,
        fixture.repository().join(".git").to_str().unwrap()
    );
    assert_eq!(
        topology.repository.primary_path,
        fixture.repository().to_str().unwrap()
    );
    assert_eq!(topology.repository.display_name, "repo");

    let worktrees = list_repository_worktrees(&pool, topology.repository.id, false).unwrap();
    let main = worktrees
        .iter()
        .find(|row| row.path == fixture.repository().to_str().unwrap())
        .unwrap();
    let linked = worktrees
        .iter()
        .find(|row| row.path == fixture.linked_worktree().to_str().unwrap())
        .unwrap();
    let detached = worktrees
        .iter()
        .find(|row| row.path == fixture.detached_worktree().to_str().unwrap())
        .unwrap();
    assert_eq!(
        main.head_sha.as_deref(),
        Some(fixture.commits.main.as_str())
    );
    assert_eq!(main.branch_name.as_deref(), Some("main"));
    assert!(!main.dirty);
    assert_eq!(
        linked.head_sha.as_deref(),
        Some(fixture.commits.feature.as_str())
    );
    assert!(linked.locked);
    assert_eq!(linked.lock_reason.as_deref(), Some("fixture lock"));
    assert_eq!(linked.branch_name.as_deref(), Some("feature"));
    assert_eq!(
        detached.head_sha.as_deref(),
        Some(fixture.commits.root.as_str())
    );
    assert!(detached.detached);
    assert_eq!(detached.branch_name, None);
    assert!(worktrees.iter().all(|row| row.status_hash.is_some()));

    let second = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T13:01:00.000Z",
    )
    .await
    .unwrap();
    assert!(second.warnings.is_empty());
    assert!(second.inserted_observations.is_empty());
    let repository = get_repository_by_key(&pool, &topology.repository.repository_key)
        .unwrap()
        .unwrap();
    assert_eq!(repository.first_seen_at, "2026-08-03T13:00:00.000Z");
    assert_eq!(repository.last_seen_at, "2026-08-03T13:01:00.000Z");
    assert!(
        list_repository_worktrees(&pool, repository.id, false)
            .unwrap()
            .iter()
            .all(|row| row.last_seen_at == "2026-08-03T13:01:00.000Z")
    );
    assert_eq!(
        list_repository_observations(&pool, repository.id)
            .unwrap()
            .len(),
        7
    );

    fs::write(
        fixture.repository().join("tracked.txt"),
        "root
dirty
",
    )
    .unwrap();
    let dirty = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T13:02:00.000Z",
    )
    .await
    .unwrap();
    assert_eq!(dirty.inserted_observations.len(), 1);
    assert_eq!(
        dirty.inserted_observations[0].observation_kind,
        RepositoryObservationKind::Status
    );

    fixture
        .git_text(fixture.repository(), &["add", "tracked.txt"])
        .unwrap();
    fixture
        .git_text(
            fixture.repository(),
            &["commit", "-m", "fixture observer head"],
        )
        .unwrap();
    let new_head = fixture
        .git_text(fixture.repository(), &["rev-parse", "HEAD"])
        .unwrap();
    let committed = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T13:03:00.000Z",
    )
    .await
    .unwrap();
    assert_eq!(committed.inserted_observations.len(), 2);
    assert!(committed.inserted_observations.iter().any(|row| {
        row.observation_kind == RepositoryObservationKind::Head
            && row.old_head_sha.as_deref() == Some(fixture.commits.main.as_str())
            && row.new_head_sha.as_deref() == Some(new_head.as_str())
    }));
    assert_eq!(
        list_repository_observations(&pool, repository.id)
            .unwrap()
            .len(),
        10
    );
}

#[derive(Default)]
struct FailOnStatusRunner {
    inner: ProcessGitRunner,
}

impl GitCommandRunner for FailOnStatusRunner {
    async fn run(&mut self, args: Vec<String>, timeout: Duration) -> Result<GitCommandResult> {
        if args.iter().any(|argument| argument == "status") {
            bail!("git timed out after {}ms", timeout.as_millis());
        }
        self.inner.run(args, timeout).await
    }
}

#[tokio::test]
async fn command_failure_returns_health_warning_without_mutating_prior_state() {
    if !git_available() {
        eprintln!("skipping repository reconcile test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(db_dir.path().join("failure.db"))).unwrap();
    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T14:00:00.000Z",
    )
    .await
    .unwrap();
    let repository = initial.topology.as_ref().unwrap().repository.clone();
    let before_worktrees = list_repository_worktrees(&pool, repository.id, true).unwrap();
    let before_observations = list_repository_observations(&pool, repository.id).unwrap();

    let mut runner = FailOnStatusRunner::default();
    let failed = reconcile_one_repository_with_runner(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T14:01:00.000Z",
        &mut runner,
    )
    .await
    .unwrap();
    assert!(failed.topology.is_none());
    assert!(failed.inserted_observations.is_empty());
    assert_eq!(failed.warnings.len(), 1);
    assert_eq!(failed.warnings[0].stage, ReconcileStage::Status);
    assert_eq!(
        failed.warnings[0].kind,
        ReconcileWarningKind::ExecutionFailed
    );

    let after_repository = get_repository_by_key(&pool, &repository.repository_key)
        .unwrap()
        .unwrap();
    assert_eq!(after_repository.last_seen_at, repository.last_seen_at);
    assert_eq!(
        list_repository_worktrees(&pool, repository.id, true).unwrap(),
        before_worktrees
    );
    assert_eq!(
        list_repository_observations(&pool, repository.id).unwrap(),
        before_observations
    );
}

#[test]
fn reconcile_options_reject_empty_hostname_and_noncanonical_input_before_git() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("validation.db"))).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut runner = FailOnStatusRunner::default();
    let error = runtime
        .block_on(reconcile_one_repository_with_runner(
            &pool,
            Path::new("relative/repository"),
            &ReconcileOptions {
                hostname: " ".to_string(),
                command_timeout: Duration::from_secs(1),
            },
            "2026-08-03T15:00:00.000Z",
            &mut runner,
        ))
        .unwrap_err();
    assert!(error.to_string().contains("hostname"));
}
