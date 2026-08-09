use super::{
    GitCommitReachabilityUpdate, GitCommitUpsert, get_git_commit, list_git_commits,
    reconcile_git_commits, upsert_git_commits,
};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{RepositoryUpsert, reconcile_repository};
use crate::db::init_pool;

const SHA_ONE: &str = "0123456789012345678901234567890123456789";
const SHA_TWO: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

fn repository() -> RepositoryUpsert {
    RepositoryUpsert {
        repository_key: "repo-key".to_string(),
        hostname: "devhost".to_string(),
        common_git_dir: "/workspace/cortex/.git".to_string(),
        primary_path: "/workspace/cortex".to_string(),
        display_name: "cortex".to_string(),
        remote_url_hash: None,
        metadata_json: "{}".to_string(),
    }
}

fn commit(sha: &str, parents: &str, subject: &str) -> GitCommitUpsert {
    GitCommitUpsert {
        sha: sha.to_string(),
        parent_shas_json: parents.to_string(),
        author_name: Some("Cortex Fixture".to_string()),
        author_email_hash: Some("sha256:fixture".to_string()),
        authored_at: Some("2026-08-04T13:00:00.000Z".to_string()),
        committed_at: Some("2026-08-04T13:00:00.000Z".to_string()),
        subject: subject.to_string(),
        changed_files: Some(2),
        insertions: Some(3),
        deletions: Some(1),
        changed_paths_json: r#"[{"path_hex":"7372632f6c69622e7273"}]"#.to_string(),
        reachable: true,
        metadata_json: r#"{"binary_files":0}"#.to_string(),
    }
}

#[test]
fn commit_upserts_preserve_identity_first_seen_order_and_exact_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("commits.db"))).unwrap();
    let repo = reconcile_repository(&pool, &repository(), &[], "2026-08-04T13:00:00.000Z")
        .unwrap()
        .repository;

    let first = upsert_git_commits(
        &pool,
        "repo-key",
        &[
            commit(SHA_ONE, "[]", "one"),
            commit(SHA_TWO, &format!(r#"["{SHA_ONE}"]"#), "two"),
        ],
        "2026-08-04T13:01:00.000Z",
    )
    .unwrap();
    assert_eq!(
        first.iter().map(|row| row.sha.as_str()).collect::<Vec<_>>(),
        vec![SHA_ONE, SHA_TWO]
    );
    assert_eq!(first[0].parent_shas_json, "[]");
    assert_eq!(first[1].parent_shas_json, format!(r#"["{SHA_ONE}"]"#));
    assert_eq!(first[0].changed_files, Some(2));
    assert_eq!(first[0].insertions, Some(3));
    assert_eq!(first[0].deletions, Some(1));
    assert!(first.iter().all(|row| row.reachable));

    let first_id = first[0].id;
    let first_seen = first[0].first_observed_at.clone();
    let mut enriched = commit(SHA_ONE, "[]", "one enriched");
    enriched.changed_files = Some(4);
    enriched.metadata_json = r#"{"binary_files":1}"#.to_string();
    let second =
        upsert_git_commits(&pool, "repo-key", &[enriched], "2026-08-04T13:02:00.000Z").unwrap();
    assert_eq!(second[0].id, first_id);
    assert_eq!(second[0].first_observed_at, first_seen);
    assert_eq!(second[0].last_observed_at, "2026-08-04T13:02:00.000Z");
    assert_eq!(second[0].subject, "one enriched");
    assert_eq!(second[0].changed_files, Some(4));

    let listed = list_git_commits(&pool, repo.id).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, first_id);
    assert_eq!(listed[1].sha, SHA_TWO);
    assert_eq!(
        get_git_commit(&pool, repo.id, SHA_ONE).unwrap().unwrap(),
        second[0]
    );
}

#[test]
fn invalid_commit_batch_rolls_back_without_partial_rows() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("rollback.db"))).unwrap();
    let repo = reconcile_repository(&pool, &repository(), &[], "2026-08-04T13:00:00.000Z")
        .unwrap()
        .repository;
    let mut invalid = commit(SHA_TWO, "[]", "invalid");
    invalid.changed_paths_json = "{".to_string();
    let error = upsert_git_commits(
        &pool,
        "repo-key",
        &[commit(SHA_ONE, "[]", "valid"), invalid],
        "2026-08-04T13:01:00.000Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("changed_paths_json"));
    assert!(list_git_commits(&pool, repo.id).unwrap().is_empty());
}

#[test]
fn reachability_updates_are_atomic_and_preserve_commit_history() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("reachability.db"))).unwrap();
    let repository = reconcile_repository(&pool, &repository(), &[], "2026-08-04T14:00:00.000Z")
        .unwrap()
        .repository;
    let initial = upsert_git_commits(
        &pool,
        "repo-key",
        &[
            commit(SHA_ONE, "[]", "one"),
            commit(SHA_TWO, &format!(r#"["{SHA_ONE}"]"#), "two"),
        ],
        "2026-08-04T14:01:00.000Z",
    )
    .unwrap();
    let second_id = initial[1].id;
    let second_first_seen = initial[1].first_observed_at.clone();

    let updated = reconcile_git_commits(
        &pool,
        "repo-key",
        &[],
        &[GitCommitReachabilityUpdate {
            sha: SHA_TWO.to_string(),
            reachable: false,
        }],
        "2026-08-04T14:02:00.000Z",
    )
    .unwrap();
    assert!(updated.is_empty());
    let unreachable = get_git_commit(&pool, repository.id, SHA_TWO)
        .unwrap()
        .unwrap();
    assert_eq!(unreachable.id, second_id);
    assert_eq!(unreachable.first_observed_at, second_first_seen);
    assert_eq!(unreachable.last_observed_at, "2026-08-04T14:02:00.000Z");
    assert!(!unreachable.reachable);

    reconcile_git_commits(
        &pool,
        "repo-key",
        &[],
        &[GitCommitReachabilityUpdate {
            sha: SHA_TWO.to_string(),
            reachable: true,
        }],
        "2026-08-04T14:03:00.000Z",
    )
    .unwrap();
    assert!(
        get_git_commit(&pool, repository.id, SHA_TWO)
            .unwrap()
            .unwrap()
            .reachable
    );

    let sha_three = "3333333333333333333333333333333333333333";
    let missing = "4444444444444444444444444444444444444444";
    let error = reconcile_git_commits(
        &pool,
        "repo-key",
        &[commit(sha_three, "[]", "three")],
        &[GitCommitReachabilityUpdate {
            sha: missing.to_string(),
            reachable: false,
        }],
        "2026-08-04T14:04:00.000Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains(missing));
    assert!(
        get_git_commit(&pool, repository.id, sha_three)
            .unwrap()
            .is_none()
    );
    assert_eq!(list_git_commits(&pool, repository.id).unwrap().len(), 2);
}
