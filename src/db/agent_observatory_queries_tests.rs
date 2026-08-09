use super::{
    RepositoryUpsert, RepositoryWorktreeUpsert, get_repository_by_key, get_worktree_by_key,
    list_repository_worktrees, mark_repository_removed, mark_worktree_removed,
    reconcile_repository,
};
use crate::config::StorageConfig;
use crate::db::init_pool;

fn repository(key: &str, display_name: &str) -> RepositoryUpsert {
    RepositoryUpsert {
        repository_key: key.to_string(),
        hostname: "devhost".to_string(),
        common_git_dir: format!("/workspace/{key}/.git"),
        primary_path: format!("/workspace/{key}"),
        display_name: display_name.to_string(),
        remote_url_hash: Some(format!("hash-{key}")),
        metadata_json: "{\"source\":\"fixture\"}".to_string(),
    }
}

fn worktree(key: &str, path: &str, branch: &str) -> RepositoryWorktreeUpsert {
    RepositoryWorktreeUpsert {
        worktree_key: key.to_string(),
        hostname: "devhost".to_string(),
        path: path.to_string(),
        git_dir: format!("{path}/.git"),
        branch_ref: Some(format!("refs/heads/{branch}")),
        branch_name: Some(branch.to_string()),
        head_sha: Some("0123456789012345678901234567890123456789".to_string()),
        upstream_ref: Some(format!("refs/remotes/origin/{branch}")),
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
        ahead: Some(0),
        behind: Some(0),
        status_hash: Some(format!("status-{key}")),
    }
}

#[test]
fn reconcile_create_update_remove_and_reappear_preserves_identity_history() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("reconcile.db"))).unwrap();
    let first_seen = "2026-08-02T16:00:00.000Z";
    let second_seen = "2026-08-02T16:01:00.000Z";
    let removed_at = "2026-08-02T16:02:00.000Z";
    let reappeared_at = "2026-08-02T16:03:00.000Z";

    let repo = repository("cortex", "Cortex");
    let primary = worktree("cortex-main", "/workspace/cortex", "main");
    let feature = worktree(
        "cortex-feature",
        "/workspace/cortex/.worktrees/feature",
        "feature",
    );
    let created = reconcile_repository(
        &pool,
        &repo,
        &[primary.clone(), feature.clone()],
        first_seen,
    )
    .unwrap();
    assert_eq!(created.worktrees.len(), 2);
    assert!(created.removed_worktree_ids.is_empty());
    let repository_id = created.repository.id;
    let repository_first_seen = created.repository.first_seen_at.clone();
    let primary_id = get_worktree_by_key(&pool, "cortex-main")
        .unwrap()
        .unwrap()
        .id;
    let feature_before = get_worktree_by_key(&pool, "cortex-feature")
        .unwrap()
        .unwrap();

    let mut updated_repo = repo.clone();
    updated_repo.primary_path = "/workspace/cortex-renamed".to_string();
    updated_repo.display_name = "Cortex Prime".to_string();
    updated_repo.remote_url_hash = None;
    updated_repo.metadata_json = "{\"source\":\"second-reconcile\"}".to_string();
    let mut updated_primary = primary.clone();
    updated_primary.branch_name = Some("trunk".to_string());
    updated_primary.branch_ref = Some("refs/heads/trunk".to_string());
    updated_primary.dirty = true;
    updated_primary.staged_count = 2;
    updated_primary.unstaged_count = 3;
    updated_primary.untracked_count = 4;
    updated_primary.ahead = Some(5);
    updated_primary.behind = None;
    updated_primary.status_hash = Some("status-updated".to_string());

    let updated =
        reconcile_repository(&pool, &updated_repo, &[updated_primary], second_seen).unwrap();
    assert_eq!(updated.repository.id, repository_id);
    assert_eq!(updated.repository.first_seen_at, repository_first_seen);
    assert_eq!(updated.repository.last_seen_at, second_seen);
    assert_eq!(updated.repository.primary_path, "/workspace/cortex-renamed");
    assert_eq!(updated.repository.display_name, "Cortex Prime");
    assert_eq!(updated.repository.remote_url_hash, None);
    assert_eq!(updated.removed_worktree_ids, vec![feature_before.id]);

    let primary_after = get_worktree_by_key(&pool, "cortex-main").unwrap().unwrap();
    assert_eq!(primary_after.id, primary_id);
    assert_eq!(primary_after.first_seen_at, first_seen);
    assert_eq!(primary_after.last_seen_at, second_seen);
    assert_eq!(primary_after.branch_name.as_deref(), Some("trunk"));
    assert!(primary_after.dirty);
    assert_eq!(primary_after.staged_count, 2);
    assert_eq!(primary_after.unstaged_count, 3);
    assert_eq!(primary_after.untracked_count, 4);
    assert_eq!(primary_after.ahead, Some(5));
    assert_eq!(primary_after.behind, None);

    let feature_removed = get_worktree_by_key(&pool, "cortex-feature")
        .unwrap()
        .unwrap();
    assert_eq!(feature_removed.id, feature_before.id);
    assert_eq!(feature_removed.first_seen_at, first_seen);
    assert_eq!(feature_removed.last_seen_at, first_seen);
    assert_eq!(feature_removed.removed_at.as_deref(), Some(second_seen));
    assert_eq!(
        list_repository_worktrees(&pool, repository_id, false)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        list_repository_worktrees(&pool, repository_id, true)
            .unwrap()
            .len(),
        2
    );

    assert!(mark_repository_removed(&pool, "cortex", removed_at).unwrap());
    assert!(mark_repository_removed(&pool, "cortex", reappeared_at).unwrap());
    let repository_removed = get_repository_by_key(&pool, "cortex").unwrap().unwrap();
    assert_eq!(repository_removed.removed_at.as_deref(), Some(removed_at));

    let reappeared = reconcile_repository(
        &pool,
        &updated_repo,
        &[primary.clone(), feature.clone()],
        reappeared_at,
    )
    .unwrap();
    assert_eq!(reappeared.repository.id, repository_id);
    assert_eq!(reappeared.repository.first_seen_at, first_seen);
    assert_eq!(reappeared.repository.removed_at, None);
    assert_eq!(reappeared.repository.last_seen_at, reappeared_at);

    let feature_after = get_worktree_by_key(&pool, "cortex-feature")
        .unwrap()
        .unwrap();
    assert_eq!(feature_after.id, feature_before.id);
    assert_eq!(feature_after.first_seen_at, first_seen);
    assert_eq!(feature_after.removed_at, None);
    assert_eq!(feature_after.last_seen_at, reappeared_at);

    assert!(mark_worktree_removed(&pool, "cortex-feature", removed_at).unwrap());
    assert!(mark_worktree_removed(&pool, "cortex-feature", reappeared_at).unwrap());
    assert_eq!(
        get_worktree_by_key(&pool, "cortex-feature")
            .unwrap()
            .unwrap()
            .removed_at
            .as_deref(),
        Some(removed_at)
    );
}

#[test]
fn reconcile_rolls_back_repository_when_worktree_upsert_fails() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("rollback.db"))).unwrap();
    let observed_at = "2026-08-02T16:00:00.000Z";

    reconcile_repository(
        &pool,
        &repository("one", "One"),
        &[worktree("shared-key", "/workspace/one", "main")],
        observed_at,
    )
    .unwrap();

    let conflict = worktree("shared-key", "/workspace/two", "main");
    assert!(
        reconcile_repository(&pool, &repository("two", "Two"), &[conflict], observed_at,).is_err()
    );
    assert!(get_repository_by_key(&pool, "two").unwrap().is_none());
    assert_eq!(
        get_worktree_by_key(&pool, "shared-key")
            .unwrap()
            .unwrap()
            .path,
        "/workspace/one"
    );
}

#[test]
fn canonical_path_validation_rejects_relative_and_parent_paths_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("paths.db"))).unwrap();
    let observed_at = "2026-08-02T16:00:00.000Z";

    let mut relative = repository("relative", "Relative");
    relative.primary_path = "workspace/relative".to_string();
    assert!(reconcile_repository(&pool, &relative, &[], observed_at).is_err());
    assert!(get_repository_by_key(&pool, "relative").unwrap().is_none());

    let mut parent = repository("parent", "Parent");
    parent.common_git_dir = "/workspace/parent/../other/.git".to_string();
    assert!(reconcile_repository(&pool, &parent, &[], observed_at).is_err());
    assert!(get_repository_by_key(&pool, "parent").unwrap().is_none());

    let repo = repository("valid", "Valid");
    let mut invalid_worktree = worktree("invalid-wt", "/workspace/valid", "main");
    invalid_worktree.git_dir = "/workspace/valid/../escape/.git".to_string();
    assert!(reconcile_repository(&pool, &repo, &[invalid_worktree], observed_at).is_err());
    assert!(get_repository_by_key(&pool, "valid").unwrap().is_none());
}

#[test]
fn parameterized_upserts_preserve_sql_metacharacters_as_data() {
    let dir = tempfile::tempdir().unwrap();
    let pool = init_pool(&StorageConfig::for_test(dir.path().join("parameters.db"))).unwrap();
    let observed_at = "2026-08-02T16:00:00.000Z";
    let display_name = "Cortex'); DROP TABLE repositories; --";
    let repo = repository("quoted", display_name);

    reconcile_repository(&pool, &repo, &[], observed_at).unwrap();
    assert_eq!(
        get_repository_by_key(&pool, "quoted")
            .unwrap()
            .unwrap()
            .display_name,
        display_name
    );

    reconcile_repository(
        &pool,
        &repository("second", "Still exists"),
        &[],
        observed_at,
    )
    .unwrap();
    assert!(get_repository_by_key(&pool, "second").unwrap().is_some());
}
