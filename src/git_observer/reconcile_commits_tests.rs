use super::{ReconcileOptions, ReconcileStage, ReconcileWarningKind, reconcile_one_repository};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    RepositoryObservationKind, get_worktree_by_key, list_git_commits,
};
use crate::db::init_pool;
use crate::git_observer::test_support::{GitFixture, git_available};
use std::fs;
use std::time::Duration;

fn options(max_commits_per_transition: usize) -> ReconcileOptions {
    ReconcileOptions {
        hostname: "dookie".to_string(),
        command_timeout: Duration::from_secs(5),
        max_commits_per_transition,
        store_changed_paths: true,
        store_author_name: true,
        store_author_email_hash: false,
    }
}

fn create_commit(fixture: &GitFixture, name: &str, subject: &str) -> String {
    fs::write(
        fixture.repository().join(name),
        format!(
            "{subject}
"
        ),
    )
    .unwrap();
    fixture
        .git_text(fixture.repository(), &["add", name])
        .unwrap();
    fixture
        .git_text(fixture.repository(), &["commit", "-m", subject])
        .unwrap();
    fixture
        .git_text(fixture.repository(), &["rev-parse", "HEAD"])
        .unwrap()
}

#[tokio::test]
async fn two_commit_fast_forward_imports_exact_commits_once_in_order() {
    if !git_available() {
        eprintln!("skipping fast-forward import test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("fast-forward.db"))).unwrap();
    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:00:00.000Z",
    )
    .await
    .unwrap();
    let repository = initial.topology.unwrap().repository;
    let first_sha = create_commit(&fixture, "ff-one.txt", "fast forward one");
    let second_sha = create_commit(&fixture, "ff-two.txt", "fast forward two");

    let imported = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:01:00.000Z",
    )
    .await
    .unwrap();
    assert!(imported.warnings.is_empty());
    assert_eq!(
        imported
            .imported_commits
            .iter()
            .map(|row| row.sha.as_str())
            .collect::<Vec<_>>(),
        vec![first_sha.as_str(), second_sha.as_str()]
    );
    assert_eq!(imported.imported_commits[0].subject, "fast forward one");
    assert_eq!(imported.imported_commits[1].subject, "fast forward two");
    assert_eq!(
        imported.imported_commits[0].parent_shas_json,
        format!(r#"["{}"]"#, fixture.commits.main)
    );
    assert_eq!(
        imported.imported_commits[1].parent_shas_json,
        format!(r#"["{first_sha}"]"#)
    );
    let heads = imported
        .inserted_observations
        .iter()
        .filter(|row| row.observation_kind == RepositoryObservationKind::Head)
        .collect::<Vec<_>>();
    assert_eq!(heads.len(), 1);
    assert_eq!(
        heads[0].old_head_sha.as_deref(),
        Some(fixture.commits.main.as_str())
    );
    assert_eq!(heads[0].new_head_sha.as_deref(), Some(second_sha.as_str()));

    let rows = list_git_commits(&pool, repository.id).unwrap();
    assert_eq!(rows, imported.imported_commits);
    let repeated = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T14:02:00.000Z",
    )
    .await
    .unwrap();
    assert!(repeated.imported_commits.is_empty());
    assert!(repeated.inserted_observations.is_empty());
    assert_eq!(list_git_commits(&pool, repository.id).unwrap(), rows);
}

#[tokio::test]
async fn transition_cap_warns_without_advancing_head_or_importing_partial_commits() {
    if !git_available() {
        eprintln!("skipping fast-forward cap test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("cap.db"))).unwrap();
    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(8),
        "2026-08-04T15:00:00.000Z",
    )
    .await
    .unwrap();
    let topology = initial.topology.unwrap();
    let main = topology
        .worktrees
        .iter()
        .find(|row| row.path == fixture.repository().to_str().unwrap())
        .unwrap()
        .clone();
    create_commit(&fixture, "cap-one.txt", "cap one");
    create_commit(&fixture, "cap-two.txt", "cap two");

    let capped = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(1),
        "2026-08-04T15:01:00.000Z",
    )
    .await
    .unwrap();
    assert!(capped.topology.is_none());
    assert!(capped.imported_commits.is_empty());
    assert!(capped.inserted_observations.is_empty());
    assert_eq!(capped.warnings.len(), 1);
    assert_eq!(capped.warnings[0].stage, ReconcileStage::CommitTraversal);
    assert_eq!(
        capped.warnings[0].kind,
        ReconcileWarningKind::CommitLimitReached { limit: 1 }
    );
    let persisted = get_worktree_by_key(&pool, &main.worktree_key)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.head_sha, main.head_sha);
    assert!(
        list_git_commits(&pool, topology.repository.id)
            .unwrap()
            .is_empty()
    );
}
