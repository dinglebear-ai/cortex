use super::{
    RepositoryObservationInput, list_repository_observations,
    record_repository_observations_if_changed,
};
use crate::config::StorageConfig;
use crate::db::agent_observatory::{
    RepositoryObservationKind, RepositoryUpsert, RepositoryWorktreeUpsert, reconcile_repository,
};
use crate::db::init_pool;
use std::collections::HashSet;

const SHA_ONE: &str = "0123456789012345678901234567890123456789";
const SHA_TWO: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

fn repository() -> RepositoryUpsert {
    RepositoryUpsert {
        repository_key: "repo-key".to_string(),
        hostname: "dookie".to_string(),
        common_git_dir: "/workspace/cortex/.git".to_string(),
        primary_path: "/workspace/cortex".to_string(),
        display_name: "cortex".to_string(),
        remote_url_hash: None,
        metadata_json: r#"{"source":"test"}"#.to_string(),
    }
}

fn worktree() -> RepositoryWorktreeUpsert {
    RepositoryWorktreeUpsert {
        worktree_key: "worktree-key".to_string(),
        hostname: "dookie".to_string(),
        path: "/workspace/cortex".to_string(),
        git_dir: "/workspace/cortex/.git".to_string(),
        branch_ref: Some("refs/heads/main".to_string()),
        branch_name: Some("main".to_string()),
        head_sha: Some(SHA_ONE.to_string()),
        upstream_ref: None,
        detached: false,
        bare: false,
        locked: false,
        lock_reason: None,
        prunable: false,
        prune_reason: None,
        dirty: false,
        staged_count: 0,
        unstaged_count: 0,
        untracked_count: 0,
        ahead: None,
        behind: None,
        status_hash: Some("status-one".to_string()),
    }
}

fn input(
    worktree_key: Option<&str>,
    observation_kind: RepositoryObservationKind,
    new_head_sha: Option<&str>,
    summary: &str,
    payload_json: &str,
) -> RepositoryObservationInput {
    RepositoryObservationInput {
        worktree_key: worktree_key.map(str::to_string),
        observation_kind,
        new_head_sha: new_head_sha.map(str::to_string),
        summary: summary.to_string(),
        payload_json: payload_json.to_string(),
    }
}

#[test]
fn observation_batch_records_only_state_changes_and_chains_head_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("observations.db"))).unwrap();
    let topology = reconcile_repository(
        &pool,
        &repository(),
        &[worktree()],
        "2026-08-03T12:00:00.000Z",
    )
    .unwrap();

    let initial = vec![
        input(
            None,
            RepositoryObservationKind::Discovered,
            None,
            "repository discovered",
            r#"{"primary_path":"/workspace/cortex"}"#,
        ),
        input(
            Some("worktree-key"),
            RepositoryObservationKind::Status,
            None,
            "worktree status changed",
            r#"{"dirty":false}"#,
        ),
        input(
            Some("worktree-key"),
            RepositoryObservationKind::Head,
            Some(SHA_ONE),
            "worktree HEAD changed",
            r#"{"head_sha":"0123456789012345678901234567890123456789"}"#,
        ),
    ];
    let inserted = record_repository_observations_if_changed(
        &pool,
        "repo-key",
        &initial,
        "2026-08-03T12:00:00.000Z",
    )
    .unwrap();
    assert_eq!(inserted.len(), 3);

    let unchanged = record_repository_observations_if_changed(
        &pool,
        "repo-key",
        &initial,
        "2026-08-03T12:01:00.000Z",
    )
    .unwrap();
    assert!(unchanged.is_empty());

    let changed = record_repository_observations_if_changed(
        &pool,
        "repo-key",
        &[
            input(
                Some("worktree-key"),
                RepositoryObservationKind::Status,
                None,
                "worktree status changed",
                r#"{"dirty":true}"#,
            ),
            input(
                Some("worktree-key"),
                RepositoryObservationKind::Head,
                Some(SHA_TWO),
                "worktree HEAD changed",
                r#"{"head_sha":"abcdefabcdefabcdefabcdefabcdefabcdefabcd"}"#,
            ),
        ],
        "2026-08-03T12:02:00.000Z",
    )
    .unwrap();
    assert_eq!(changed.len(), 2);

    let reverted = record_repository_observations_if_changed(
        &pool,
        "repo-key",
        &[input(
            Some("worktree-key"),
            RepositoryObservationKind::Head,
            Some(SHA_ONE),
            "worktree HEAD changed",
            r#"{"head_sha":"0123456789012345678901234567890123456789"}"#,
        )],
        "2026-08-03T12:03:00.000Z",
    )
    .unwrap();
    assert_eq!(reverted.len(), 1);

    let rows = list_repository_observations(&pool, topology.repository.id).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows.iter()
            .map(|row| row.observation_key.as_str())
            .collect::<HashSet<_>>()
            .len(),
        rows.len()
    );
    let heads = rows
        .iter()
        .filter(|row| row.observation_kind == RepositoryObservationKind::Head)
        .collect::<Vec<_>>();
    assert_eq!(heads.len(), 3);
    assert_eq!(heads[0].old_head_sha, None);
    assert_eq!(heads[0].new_head_sha.as_deref(), Some(SHA_ONE));
    assert_eq!(heads[1].old_head_sha.as_deref(), Some(SHA_ONE));
    assert_eq!(heads[1].new_head_sha.as_deref(), Some(SHA_TWO));
    assert_eq!(heads[2].old_head_sha.as_deref(), Some(SHA_TWO));
    assert_eq!(heads[2].new_head_sha.as_deref(), Some(SHA_ONE));
}

#[test]
fn invalid_observation_batch_rolls_back_every_insert() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("rollback.db"))).unwrap();
    let topology = reconcile_repository(
        &pool,
        &repository(),
        &[worktree()],
        "2026-08-03T12:00:00.000Z",
    )
    .unwrap();

    let error = record_repository_observations_if_changed(
        &pool,
        "repo-key",
        &[
            input(
                None,
                RepositoryObservationKind::Discovered,
                None,
                "repository discovered",
                "{}",
            ),
            input(
                Some("missing-worktree"),
                RepositoryObservationKind::Status,
                None,
                "worktree status changed",
                "{}",
            ),
        ],
        "2026-08-03T12:01:00.000Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains("missing-worktree"));
    assert!(
        list_repository_observations(&pool, topology.repository.id)
            .unwrap()
            .is_empty()
    );
}
