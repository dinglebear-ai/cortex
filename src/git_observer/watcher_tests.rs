use super::{
    RepositoryWatchInput, WatchPlanErrorKind, WatchPlannerOptions, WorktreeWatchInput,
    plan_watch_set,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

struct WatchFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repository: PathBuf,
    common: PathBuf,
    linked: PathBuf,
    linked_control: PathBuf,
    detached: PathBuf,
    detached_control: PathBuf,
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

impl WatchFixture {
    fn build() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical(temp.path());
        let repository = root.join("repo");
        let common = repository.join(".git");
        let linked = root.join("linked");
        let detached = root.join("detached");
        let linked_control = common.join("worktrees/linked");
        let detached_control = common.join("worktrees/detached");
        for path in [
            &repository,
            &common,
            &linked,
            &detached,
            &linked_control,
            &detached_control,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        Self {
            _temp: temp,
            root,
            repository,
            common: canonical(&common),
            linked: canonical(&linked),
            linked_control: canonical(&linked_control),
            detached: canonical(&detached),
            detached_control: canonical(&detached_control),
        }
    }

    fn repository_input(&self) -> RepositoryWatchInput {
        RepositoryWatchInput {
            repository_key: "repo-key".to_string(),
            common_git_dir: self.common.clone(),
            worktrees: vec![
                WorktreeWatchInput {
                    worktree_path: self.repository.clone(),
                    control_dir: self.common.clone(),
                },
                WorktreeWatchInput {
                    worktree_path: self.linked.clone(),
                    control_dir: self.linked_control.clone(),
                },
                WorktreeWatchInput {
                    worktree_path: self.detached.clone(),
                    control_dir: self.detached_control.clone(),
                },
            ],
        }
    }

    fn expected_paths(&self) -> BTreeSet<PathBuf> {
        [
            self.root.clone(),
            self.common.clone(),
            self.common.join("HEAD"),
            self.common.join("index"),
            self.common.join("packed-refs"),
            self.common.join("refs"),
            self.common.join("worktrees"),
            self.linked_control.clone(),
            self.linked_control.join("HEAD"),
            self.linked_control.join("index"),
            self.detached_control.clone(),
            self.detached_control.join("HEAD"),
            self.detached_control.join("index"),
        ]
        .into_iter()
        .collect()
    }
}

fn options(max_paths: usize) -> WatchPlannerOptions {
    WatchPlannerOptions { max_paths }
}

#[test]
fn planner_returns_only_project_roots_and_git_control_paths() {
    let fixture = WatchFixture::build();
    let plan = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[fixture.repository_input()],
        options(64),
    )
    .unwrap();

    assert_eq!(
        plan.targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<BTreeSet<_>>(),
        fixture.expected_paths()
    );
    assert_eq!(plan.targets.len(), 13);
    assert!(
        plan.targets
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    );

    let root = plan
        .targets
        .iter()
        .find(|target| target.path == fixture.root)
        .unwrap();
    assert!(root.discovers_repositories);
    assert!(root.repository_keys.is_empty());

    let common = plan
        .targets
        .iter()
        .find(|target| target.path == fixture.common)
        .unwrap();
    assert!(!common.discovers_repositories);
    assert_eq!(common.repository_keys, vec!["repo-key"]);

    assert!(
        plan.targets.iter().all(|target| {
            target.path == fixture.root || target.path.starts_with(&fixture.common)
        })
    );
    assert!(
        !plan
            .targets
            .iter()
            .any(|target| target.path.starts_with(fixture.repository.join("src")))
    );
}

#[test]
fn ten_thousand_source_files_do_not_change_watch_count_or_targets() {
    let fixture = WatchFixture::build();
    let input = fixture.repository_input();
    let before = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        std::slice::from_ref(&input),
        options(64),
    )
    .unwrap();

    let source = fixture.repository.join("src");
    fs::create_dir_all(&source).unwrap();
    for index in 0..10_000 {
        fs::write(
            source.join(format!("file-{index:05}.rs")),
            b"fn fixture() {}
",
        )
        .unwrap();
    }

    let after = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        std::slice::from_ref(&input),
        options(64),
    )
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.targets.len(), 13);
}

#[test]
fn planner_is_deterministic_deduplicated_and_hard_capped() {
    let fixture = WatchFixture::build();
    let forward = fixture.repository_input();
    let mut reversed = fixture.repository_input();
    reversed.worktrees.reverse();

    let first = plan_watch_set(
        &[fixture.root.clone(), fixture.root.clone()],
        &[forward],
        options(13),
    )
    .unwrap();
    let second = plan_watch_set(
        &[fixture.root.clone(), fixture.root.clone()],
        &[reversed],
        options(13),
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.targets.len(), 13);

    let error = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[fixture.repository_input()],
        options(12),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        WatchPlanErrorKind::PathLimitReached {
            limit: 12,
            observed: 13,
        }
    );
    assert!(error.to_string().len() < 160);
}

#[test]
fn canonical_and_containment_rules_reject_unsafe_inputs() {
    let fixture = WatchFixture::build();

    let error = plan_watch_set(
        &[PathBuf::from("relative/root")],
        &[fixture.repository_input()],
        options(64),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        WatchPlanErrorKind::NonCanonicalPath("project_root")
    );

    let error = plan_watch_set(
        &[fixture.root.join("repo/../repo")],
        &[fixture.repository_input()],
        options(64),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        WatchPlanErrorKind::NonCanonicalPath("project_root")
    );

    let outside = tempfile::tempdir().unwrap();
    let outside_path = canonical(outside.path());
    let mut outside_worktree = fixture.repository_input();
    outside_worktree.worktrees[1].worktree_path = outside_path.clone();
    let error = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[outside_worktree],
        options(64),
    )
    .unwrap_err();
    assert_eq!(error.kind, WatchPlanErrorKind::WorktreeOutsideProjectRoots);

    let mut outside_control = fixture.repository_input();
    outside_control.worktrees[1].control_dir = outside_path;
    let error = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[outside_control],
        options(64),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        WatchPlanErrorKind::ControlDirectoryOutsideCommonGitDir
    );

    let mut blank_key = fixture.repository_input();
    blank_key.repository_key = " ".to_string();
    let error = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[blank_key],
        options(64),
    )
    .unwrap_err();
    assert_eq!(error.kind, WatchPlanErrorKind::EmptyRepositoryKey);
}

#[test]
fn duplicate_repository_keys_and_zero_caps_are_rejected() {
    let fixture = WatchFixture::build();
    let input = fixture.repository_input();
    let error = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[input.clone(), input],
        options(64),
    )
    .unwrap_err();
    assert_eq!(error.kind, WatchPlanErrorKind::DuplicateRepositoryKey);

    let error = plan_watch_set(
        std::slice::from_ref(&fixture.root),
        &[fixture.repository_input()],
        options(0),
    )
    .unwrap_err();
    assert_eq!(error.kind, WatchPlanErrorKind::InvalidMaxPaths);
}
