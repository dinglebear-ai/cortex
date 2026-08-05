//! Pure bounded Git watch-set planning.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_MAX_WATCH_PATHS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchPlannerOptions {
    pub max_paths: usize,
}

impl Default for WatchPlannerOptions {
    fn default() -> Self {
        Self {
            max_paths: DEFAULT_MAX_WATCH_PATHS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeWatchInput {
    pub worktree_path: PathBuf,
    pub control_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryWatchInput {
    pub repository_key: String,
    pub common_git_dir: PathBuf,
    pub worktrees: Vec<WorktreeWatchInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchTarget {
    pub path: PathBuf,
    pub repository_keys: Vec<String>,
    pub discovers_repositories: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WatchPlan {
    pub targets: Vec<WatchTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchPlanErrorKind {
    InvalidMaxPaths,
    NoProjectRoots,
    EmptyRepositoryKey,
    DuplicateRepositoryKey,
    NonCanonicalPath(&'static str),
    WorktreeOutsideProjectRoots,
    ControlDirectoryOutsideCommonGitDir,
    PathLimitReached { limit: usize, observed: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPlanError {
    pub repository_index: Option<usize>,
    pub worktree_index: Option<usize>,
    pub kind: WatchPlanErrorKind,
}

impl WatchPlanError {
    fn root(index: usize, kind: WatchPlanErrorKind) -> Self {
        Self {
            repository_index: None,
            worktree_index: Some(index),
            kind,
        }
    }

    fn repository(index: usize, kind: WatchPlanErrorKind) -> Self {
        Self {
            repository_index: Some(index),
            worktree_index: None,
            kind,
        }
    }

    fn worktree(repository_index: usize, worktree_index: usize, kind: WatchPlanErrorKind) -> Self {
        Self {
            repository_index: Some(repository_index),
            worktree_index: Some(worktree_index),
            kind,
        }
    }

    fn global(kind: WatchPlanErrorKind) -> Self {
        Self {
            repository_index: None,
            worktree_index: None,
            kind,
        }
    }
}

impl fmt::Display for WatchPlanErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMaxPaths => formatter.write_str("max_paths must be positive"),
            Self::NoProjectRoots => formatter.write_str("at least one project root is required"),
            Self::EmptyRepositoryKey => formatter.write_str("repository key is empty"),
            Self::DuplicateRepositoryKey => formatter.write_str("repository key is duplicated"),
            Self::NonCanonicalPath(field) => write!(formatter, "{field} is not canonical"),
            Self::WorktreeOutsideProjectRoots => {
                formatter.write_str("worktree is outside configured project roots")
            }
            Self::ControlDirectoryOutsideCommonGitDir => {
                formatter.write_str("worktree control directory is outside common Git directory")
            }
            Self::PathLimitReached { limit, observed } => {
                write!(
                    formatter,
                    "watch path count {observed} exceeds limit {limit}"
                )
            }
        }
    }
}

impl fmt::Display for WatchPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Git watch plan: {}", self.kind)
    }
}

impl std::error::Error for WatchPlanError {}

#[derive(Default)]
struct TargetBuilder {
    repository_keys: BTreeSet<String>,
    discovers_repositories: bool,
}

struct PlanBuilder {
    targets: BTreeMap<PathBuf, TargetBuilder>,
    max_paths: usize,
}

impl PlanBuilder {
    fn new(max_paths: usize) -> Self {
        Self {
            targets: BTreeMap::new(),
            max_paths,
        }
    }

    fn add(
        &mut self,
        path: PathBuf,
        repository_key: Option<&str>,
        discovers_repositories: bool,
    ) -> Result<(), WatchPlanError> {
        if !self.targets.contains_key(&path) && self.targets.len() == self.max_paths {
            return Err(WatchPlanError::global(
                WatchPlanErrorKind::PathLimitReached {
                    limit: self.max_paths,
                    observed: self.max_paths + 1,
                },
            ));
        }
        let target = self.targets.entry(path).or_default();
        if let Some(repository_key) = repository_key {
            target.repository_keys.insert(repository_key.to_string());
        }
        target.discovers_repositories |= discovers_repositories;
        Ok(())
    }

    fn finish(self) -> WatchPlan {
        WatchPlan {
            targets: self
                .targets
                .into_iter()
                .map(|(path, target)| WatchTarget {
                    path,
                    repository_keys: target.repository_keys.into_iter().collect(),
                    discovers_repositories: target.discovers_repositories,
                })
                .collect(),
        }
    }
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

fn validate_path(
    path: &Path,
    field: &'static str,
    error: impl FnOnce(WatchPlanErrorKind) -> WatchPlanError,
) -> Result<(), WatchPlanError> {
    if is_canonical_absolute(path) {
        Ok(())
    } else {
        Err(error(WatchPlanErrorKind::NonCanonicalPath(field)))
    }
}

fn contained_by_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
}

fn add_common_controls(
    builder: &mut PlanBuilder,
    common: &Path,
    repository_key: &str,
) -> Result<(), WatchPlanError> {
    for path in [
        common.to_path_buf(),
        common.join("HEAD"),
        common.join("index"),
        common.join("packed-refs"),
        common.join("refs"),
        common.join("worktrees"),
    ] {
        builder.add(path, Some(repository_key), false)?;
    }
    Ok(())
}

fn add_worktree_controls(
    builder: &mut PlanBuilder,
    control: &Path,
    repository_key: &str,
) -> Result<(), WatchPlanError> {
    for path in [
        control.to_path_buf(),
        control.join("HEAD"),
        control.join("index"),
    ] {
        builder.add(path, Some(repository_key), false)?;
    }
    Ok(())
}

/// Build a deterministic watch set without reading or traversing the filesystem.
///
/// Callers provide already-canonical project roots and exact Git control
/// directories. Worktree source paths are validated for containment but are not
/// watched. A linked-worktree control directory must be a child of the common
/// Git directory's worktrees directory.
pub fn plan_watch_set(
    project_roots: &[PathBuf],
    repositories: &[RepositoryWatchInput],
    options: WatchPlannerOptions,
) -> Result<WatchPlan, WatchPlanError> {
    if options.max_paths == 0 {
        return Err(WatchPlanError::global(WatchPlanErrorKind::InvalidMaxPaths));
    }
    if project_roots.is_empty() {
        return Err(WatchPlanError::global(WatchPlanErrorKind::NoProjectRoots));
    }

    let mut builder = PlanBuilder::new(options.max_paths);
    let mut roots = Vec::new();
    for (index, root) in project_roots.iter().enumerate() {
        validate_path(root, "project_root", |kind| {
            WatchPlanError::root(index, kind)
        })?;
        if !roots.contains(root) {
            roots.push(root.clone());
        }
        builder.add(root.clone(), None, true)?;
    }
    roots.sort();

    let mut repository_keys = HashSet::new();
    for (repository_index, repository) in repositories.iter().enumerate() {
        let repository_key = repository.repository_key.trim();
        if repository_key.is_empty() {
            return Err(WatchPlanError::repository(
                repository_index,
                WatchPlanErrorKind::EmptyRepositoryKey,
            ));
        }
        if !repository_keys.insert(repository_key.to_string()) {
            return Err(WatchPlanError::repository(
                repository_index,
                WatchPlanErrorKind::DuplicateRepositoryKey,
            ));
        }
        validate_path(&repository.common_git_dir, "common_git_dir", |kind| {
            WatchPlanError::repository(repository_index, kind)
        })?;
        add_common_controls(&mut builder, &repository.common_git_dir, repository_key)?;

        let linked_controls = repository.common_git_dir.join("worktrees");
        for (worktree_index, worktree) in repository.worktrees.iter().enumerate() {
            validate_path(&worktree.worktree_path, "worktree_path", |kind| {
                WatchPlanError::worktree(repository_index, worktree_index, kind)
            })?;
            if !contained_by_any(&worktree.worktree_path, &roots) {
                return Err(WatchPlanError::worktree(
                    repository_index,
                    worktree_index,
                    WatchPlanErrorKind::WorktreeOutsideProjectRoots,
                ));
            }
            validate_path(&worktree.control_dir, "control_dir", |kind| {
                WatchPlanError::worktree(repository_index, worktree_index, kind)
            })?;
            let valid_control = worktree.control_dir == repository.common_git_dir
                || (worktree.control_dir != linked_controls
                    && worktree.control_dir.starts_with(&linked_controls));
            if !valid_control {
                return Err(WatchPlanError::worktree(
                    repository_index,
                    worktree_index,
                    WatchPlanErrorKind::ControlDirectoryOutsideCommonGitDir,
                ));
            }
            add_worktree_controls(&mut builder, &worktree.control_dir, repository_key)?;
        }
    }

    Ok(builder.finish())
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
