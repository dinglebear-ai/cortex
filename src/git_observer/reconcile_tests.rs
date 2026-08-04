use super::{
    GitCommandResult, GitCommandRunner, ProcessGitRunner, ReconcileOptions, ReconcileStage,
    ReconcileWarningKind, reconcile_one_repository, reconcile_one_repository_with_runner,
};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    RepositoryObservationKind, get_repository_by_key, get_worktree_by_key,
    list_repository_observations, list_repository_worktrees,
};
use crate::db::init_pool;
use crate::git_observer::test_support::{GitFixture, git_available};
use anyhow::{Result, bail};
use rusqlite::params;
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

#[tokio::test]
async fn removed_and_reappeared_worktree_reuses_identity_and_preserves_run_evidence() {
    if !git_available() {
        eprintln!("skipping worktree lifecycle test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(db_dir.path().join("lifecycle.db"))).unwrap();
    let first_seen = "2026-08-03T16:00:00.000Z";
    let removed_at = "2026-08-03T16:01:00.000Z";
    let reappeared_at = "2026-08-03T16:03:00.000Z";
    let removed_again_at = "2026-08-03T16:04:00.000Z";

    let initial = reconcile_one_repository(&pool, fixture.repository(), &options(), first_seen)
        .await
        .unwrap();
    let repository = initial.topology.as_ref().unwrap().repository.clone();
    let linked_before = list_repository_worktrees(&pool, repository.id, false)
        .unwrap()
        .into_iter()
        .find(|row| row.path == fixture.linked_worktree().to_str().unwrap())
        .unwrap();

    let run_id = {
        let connection = pool.get().unwrap();
        connection
            .execute(
                "INSERT INTO agent_runs
                    (run_key, native_session_id, tool, hostname, primary_worktree_id,
                     status, status_reason, status_observed_at, started_at, last_activity_at)
                 VALUES ('run-key', 'session-one', 'claude', 'dookie', ?1,
                         'active', 'fixture', ?2, ?2, ?2)",
                params![linked_before.id, first_seen],
            )
            .unwrap();
        let run_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO agent_run_worktrees
                    (relation_key, run_id, worktree_id, evidence_kind, evidence_source,
                     trust_level, confidence, is_primary, first_seen_at, last_seen_at)
                 VALUES ('relation-key', ?1, ?2, 'cwd', 'fixture',
                         'verified', 1.0, 1, ?3, ?3)",
                params![run_id, linked_before.id, first_seen],
            )
            .unwrap();
        run_id
    };

    let linked_path = fixture.linked_worktree().to_str().unwrap();
    fixture
        .git_text(fixture.repository(), &["worktree", "unlock", linked_path])
        .unwrap();
    fixture
        .git_text(
            fixture.repository(),
            &["worktree", "remove", "--force", linked_path],
        )
        .unwrap();

    let removed = reconcile_one_repository(&pool, fixture.repository(), &options(), removed_at)
        .await
        .unwrap();
    assert_eq!(
        removed.topology.as_ref().unwrap().removed_worktree_ids,
        vec![linked_before.id]
    );
    assert_eq!(removed.inserted_observations.len(), 1);
    assert_eq!(
        removed.inserted_observations[0].observation_kind,
        RepositoryObservationKind::WorktreeRemoved
    );
    let removed_row = get_worktree_by_key(&pool, &linked_before.worktree_key)
        .unwrap()
        .unwrap();
    assert_eq!(removed_row.id, linked_before.id);
    assert_eq!(removed_row.first_seen_at, linked_before.first_seen_at);
    assert_eq!(removed_row.last_seen_at, linked_before.last_seen_at);
    assert_eq!(removed_row.removed_at.as_deref(), Some(removed_at));

    let still_removed = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-03T16:02:00.000Z",
    )
    .await
    .unwrap();
    assert!(
        still_removed
            .topology
            .as_ref()
            .unwrap()
            .removed_worktree_ids
            .is_empty()
    );
    assert!(still_removed.inserted_observations.is_empty());

    fixture
        .git_text(
            fixture.repository(),
            &["worktree", "add", linked_path, "feature"],
        )
        .unwrap();
    fixture
        .git_text(
            fixture.repository(),
            &["worktree", "lock", "--reason", "fixture lock", linked_path],
        )
        .unwrap();

    let reappeared =
        reconcile_one_repository(&pool, fixture.repository(), &options(), reappeared_at)
            .await
            .unwrap();
    assert_eq!(reappeared.inserted_observations.len(), 1);
    assert_eq!(
        reappeared.inserted_observations[0].observation_kind,
        RepositoryObservationKind::WorktreeAdded
    );
    let linked_after = get_worktree_by_key(&pool, &linked_before.worktree_key)
        .unwrap()
        .unwrap();
    assert_eq!(linked_after.id, linked_before.id);
    assert_eq!(linked_after.first_seen_at, linked_before.first_seen_at);
    assert_eq!(linked_after.last_seen_at, reappeared_at);
    assert_eq!(linked_after.removed_at, None);

    fixture
        .git_text(fixture.repository(), &["worktree", "unlock", linked_path])
        .unwrap();
    fixture
        .git_text(
            fixture.repository(),
            &["worktree", "remove", "--force", linked_path],
        )
        .unwrap();
    let removed_again =
        reconcile_one_repository(&pool, fixture.repository(), &options(), removed_again_at)
            .await
            .unwrap();
    assert_eq!(removed_again.inserted_observations.len(), 1);
    assert_eq!(
        removed_again.inserted_observations[0].observation_kind,
        RepositoryObservationKind::WorktreeRemoved
    );
    let final_row = get_worktree_by_key(&pool, &linked_before.worktree_key)
        .unwrap()
        .unwrap();
    assert_eq!(final_row.id, linked_before.id);
    assert_eq!(final_row.first_seen_at, linked_before.first_seen_at);
    assert_eq!(final_row.last_seen_at, reappeared_at);
    assert_eq!(final_row.removed_at.as_deref(), Some(removed_again_at));

    let lifecycle = list_repository_observations(&pool, repository.id)
        .unwrap()
        .into_iter()
        .filter(|row| {
            matches!(
                row.observation_kind,
                RepositoryObservationKind::WorktreeAdded
                    | RepositoryObservationKind::WorktreeRemoved
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle
            .iter()
            .map(|row| row.observation_kind)
            .collect::<Vec<_>>(),
        vec![
            RepositoryObservationKind::WorktreeRemoved,
            RepositoryObservationKind::WorktreeAdded,
            RepositoryObservationKind::WorktreeRemoved,
        ]
    );

    let connection = pool.get().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT primary_worktree_id FROM agent_runs WHERE id = ?1",
                [run_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        linked_before.id
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_run_worktrees
                  WHERE run_id = ?1 AND worktree_id = ?2",
                params![run_id, linked_before.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    let mut foreign_keys = connection.prepare("PRAGMA foreign_key_check").unwrap();
    assert!(foreign_keys.query([]).unwrap().next().unwrap().is_none());
}
