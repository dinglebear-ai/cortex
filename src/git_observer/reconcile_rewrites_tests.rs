use super::{
    GitCommandResult, GitCommandRunner, ProcessGitRunner, ReconcileOptions,
    reconcile_one_repository, reconcile_one_repository_with_runner,
};
use crate::config::StorageConfig;
use crate::db::DbPool;
use crate::db::agent_observatory::{
    GitCommitRow, RepositoryObservationKind, RepositoryObservationRow, get_git_commit,
    list_git_commits, list_repository_observations,
};
use crate::db::init_pool;
use crate::git_observer::test_support::{GitFixture, git_available};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::time::Duration;

fn options() -> ReconcileOptions {
    ReconcileOptions {
        hostname: "devhost".to_string(),
        command_timeout: Duration::from_secs(5),
        max_commits_per_transition: 32,
        store_changed_paths: true,
        store_author_name: true,
        store_author_email_hash: false,
    }
}

fn commit_file(fixture: &GitFixture, name: &str, contents: &str, subject: &str) -> String {
    fs::write(fixture.repository().join(name), contents).unwrap();
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

fn commit<'a>(rows: &'a [GitCommitRow], sha: &str) -> &'a GitCommitRow {
    rows.iter().find(|row| row.sha == sha).unwrap()
}

fn last_head_observation(
    pool: &DbPool,
    repository_id: i64,
    worktree_id: i64,
) -> RepositoryObservationRow {
    list_repository_observations(pool, repository_id)
        .unwrap()
        .into_iter()
        .rfind(|row| {
            row.worktree_id == Some(worktree_id)
                && row.observation_kind == RepositoryObservationKind::Head
        })
        .unwrap()
}

fn payload(row: &RepositoryObservationRow) -> Value {
    serde_json::from_str(&row.payload_json).unwrap()
}

#[derive(Default)]
struct RecordingRunner {
    inner: ProcessGitRunner,
    commands: Vec<Vec<String>>,
}

impl GitCommandRunner for RecordingRunner {
    async fn run(&mut self, args: Vec<String>, timeout: Duration) -> Result<GitCommandResult> {
        self.commands.push(args.clone());
        self.inner.run(args, timeout).await
    }
}

#[tokio::test]
async fn rewind_rebase_and_detached_transitions_preserve_history_and_update_reachability() {
    if !git_available() {
        eprintln!("skipping non-fast-forward test: git executable is unavailable");
        return;
    }
    let fixture = GitFixture::build().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let pool = std::sync::Arc::new(
        init_pool(&StorageConfig::for_test(db_dir.path().join("rewrites.db"))).unwrap(),
    );

    let initial = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:00:00.000Z",
    )
    .await
    .unwrap();
    let repository = initial.topology.as_ref().unwrap().repository.clone();
    let main = initial
        .topology
        .as_ref()
        .unwrap()
        .worktrees
        .iter()
        .find(|row| row.path == fixture.repository().to_str().unwrap())
        .unwrap()
        .clone();

    let first = commit_file(
        &fixture,
        "ff-one.txt",
        "one
",
        "ff one",
    );
    let second = commit_file(
        &fixture,
        "ff-two.txt",
        "two
",
        "ff two",
    );
    let fast_forward = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:01:00.000Z",
    )
    .await
    .unwrap();
    assert_eq!(
        fast_forward
            .imported_commits
            .iter()
            .map(|row| row.sha.as_str())
            .collect::<Vec<_>>(),
        vec![first.as_str(), second.as_str()]
    );

    fixture
        .git_text(fixture.repository(), &["reset", "--hard", &first])
        .unwrap();
    let rewind = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:02:00.000Z",
    )
    .await
    .unwrap();
    let rewind_main = rewind
        .topology
        .as_ref()
        .unwrap()
        .worktrees
        .iter()
        .find(|row| row.id == main.id)
        .unwrap();
    assert_eq!(rewind_main.head_sha.as_deref(), Some(first.as_str()));
    let after_rewind = list_git_commits(&pool, repository.id).unwrap();
    assert!(commit(&after_rewind, &first).reachable);
    assert!(!commit(&after_rewind, &second).reachable);
    assert!(
        get_git_commit(&pool, repository.id, &second)
            .unwrap()
            .is_some()
    );
    let rewind_observation = last_head_observation(&pool, repository.id, main.id);
    assert_eq!(
        rewind_observation.old_head_sha.as_deref(),
        Some(second.as_str())
    );
    assert_eq!(
        rewind_observation.new_head_sha.as_deref(),
        Some(first.as_str())
    );
    let rewind_payload = payload(&rewind_observation);
    assert_eq!(rewind_payload["fast_forward"], false);
    assert_eq!(rewind_payload["transition_kind"], "rewind");
    assert_eq!(rewind_payload["detached"], false);
    assert_eq!(rewind_payload["new_commit_count"], 0);
    assert_eq!(rewind_payload["displaced_commit_count"], 1);

    let repeat = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:03:00.000Z",
    )
    .await
    .unwrap();
    assert!(repeat.imported_commits.is_empty());
    assert!(repeat.inserted_observations.is_empty());

    fixture
        .git_text(fixture.repository(), &["switch", "-c", "rewrite-topic"])
        .unwrap();
    let topic_old = commit_file(
        &fixture,
        "topic.txt",
        "topic
",
        "topic old",
    );
    reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:04:00.000Z",
    )
    .await
    .unwrap();

    fixture
        .git_text(fixture.repository(), &["switch", "main"])
        .unwrap();
    let base = commit_file(
        &fixture, "base.txt", "base
", "new base",
    );
    fixture
        .git_text(fixture.repository(), &["switch", "rewrite-topic"])
        .unwrap();
    fixture
        .git_text(fixture.repository(), &["rebase", "main"])
        .unwrap();
    let rewritten = fixture
        .git_text(fixture.repository(), &["rev-parse", "HEAD"])
        .unwrap();
    assert_ne!(rewritten, topic_old);

    let rewrite = reconcile_one_repository(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:05:00.000Z",
    )
    .await
    .unwrap();
    assert_eq!(
        rewrite
            .imported_commits
            .iter()
            .map(|row| row.sha.as_str())
            .collect::<Vec<_>>(),
        vec![base.as_str(), rewritten.as_str(), topic_old.as_str()]
    );
    let after_rewrite = list_git_commits(&pool, repository.id).unwrap();
    assert!(!commit(&after_rewrite, &topic_old).reachable);
    assert!(commit(&after_rewrite, &base).reachable);
    assert!(commit(&after_rewrite, &rewritten).reachable);
    assert!(!commit(&after_rewrite, &second).reachable);
    let rewrite_observation = last_head_observation(&pool, repository.id, main.id);
    assert_eq!(
        rewrite_observation.old_head_sha.as_deref(),
        Some(topic_old.as_str())
    );
    assert_eq!(
        rewrite_observation.new_head_sha.as_deref(),
        Some(rewritten.as_str())
    );
    let rewrite_payload = payload(&rewrite_observation);
    assert_eq!(rewrite_payload["fast_forward"], false);
    assert_eq!(rewrite_payload["transition_kind"], "rewrite");
    assert_eq!(rewrite_payload["detached"], false);
    assert_eq!(rewrite_payload["new_commit_count"], 2);
    assert_eq!(rewrite_payload["displaced_commit_count"], 1);

    fixture
        .git_text(fixture.repository(), &["switch", "--detach", &base])
        .unwrap();
    let mut runner = RecordingRunner::default();
    let detached = reconcile_one_repository_with_runner(
        &pool,
        fixture.repository(),
        &options(),
        "2026-08-04T15:06:00.000Z",
        &mut runner,
    )
    .await
    .unwrap();
    let detached_main = detached
        .topology
        .as_ref()
        .unwrap()
        .worktrees
        .iter()
        .find(|row| row.id == main.id)
        .unwrap();
    assert_eq!(detached_main.head_sha.as_deref(), Some(base.as_str()));
    assert!(detached_main.detached);
    let after_detached = list_git_commits(&pool, repository.id).unwrap();
    assert!(!commit(&after_detached, &rewritten).reachable);
    assert!(commit(&after_detached, &base).reachable);
    let detached_observation = last_head_observation(&pool, repository.id, main.id);
    let detached_payload = payload(&detached_observation);
    assert_eq!(detached_payload["transition_kind"], "rewind");
    assert_eq!(detached_payload["detached"], true);

    let destructive = ["reset", "rebase", "checkout", "switch", "commit", "merge"];
    assert!(
        runner
            .commands
            .iter()
            .flatten()
            .all(|argument| !destructive.contains(&argument.as_str()))
    );
    let final_rows = list_git_commits(&pool, repository.id).unwrap();
    assert!(final_rows.iter().any(|row| row.sha == second));
    assert!(final_rows.iter().any(|row| row.sha == topic_old));
    assert!(final_rows.iter().any(|row| row.sha == rewritten));
}
