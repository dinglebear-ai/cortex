//! Bounded canonical repository discovery.

use std::collections::HashSet;
use std::fs::{self, DirEntry};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryOptions {
    pub max_depth: usize,
    pub max_repositories: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_repositories: 120,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryWarningKind {
    RootUnavailable { error_kind: ErrorKind },
    ReadDirectoryFailed { error_kind: ErrorKind },
    EntryTypeFailed { error_kind: ErrorKind },
    SymlinkSkipped,
    DepthLimitReached { max_depth: usize },
    RepositoryLimitReached { limit: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryWarning {
    pub kind: DiscoveryWarningKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveryResult {
    pub repositories: Vec<PathBuf>,
    pub warnings: Vec<DiscoveryWarning>,
}

struct DiscoveryState {
    options: DiscoveryOptions,
    repositories: Vec<PathBuf>,
    seen_repositories: HashSet<PathBuf>,
    warnings: Vec<DiscoveryWarning>,
    limit_reported: bool,
}

impl DiscoveryState {
    fn new(options: DiscoveryOptions) -> Self {
        Self {
            options,
            repositories: Vec::new(),
            seen_repositories: HashSet::new(),
            warnings: Vec::new(),
            limit_reported: false,
        }
    }

    fn warning(&mut self, kind: DiscoveryWarningKind, path: PathBuf) {
        self.warnings.push(DiscoveryWarning { kind, path });
    }

    fn repository_limit(&mut self, root: &Path) {
        if !self.limit_reported {
            self.limit_reported = true;
            self.warning(
                DiscoveryWarningKind::RepositoryLimitReached {
                    limit: self.options.max_repositories,
                },
                root.to_path_buf(),
            );
        }
    }

    fn add_repository(&mut self, path: PathBuf) {
        if self.seen_repositories.insert(path.clone()) {
            self.repositories.push(path);
        }
    }

    fn has_git_marker(&mut self, path: &Path, entries: &[DirEntry]) -> bool {
        let Some(marker) = entries.iter().find(|entry| entry.file_name() == ".git") else {
            return false;
        };
        match marker.file_type() {
            Ok(kind) if kind.is_symlink() => {
                self.warning(DiscoveryWarningKind::SymlinkSkipped, path.join(".git"));
                false
            }
            Ok(kind) => kind.is_dir() || kind.is_file(),
            Err(error) => {
                self.warning(
                    DiscoveryWarningKind::EntryTypeFailed {
                        error_kind: error.kind(),
                    },
                    path.join(".git"),
                );
                false
            }
        }
    }

    fn walk_root(&mut self, root: &Path) {
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((path, depth)) = stack.pop() {
            if self.repositories.len() >= self.options.max_repositories {
                self.repository_limit(root);
                break;
            }

            let entries = match sorted_entries(&path) {
                Ok(entries) => entries,
                Err(error) => {
                    self.warning(
                        DiscoveryWarningKind::ReadDirectoryFailed {
                            error_kind: error.kind(),
                        },
                        path,
                    );
                    continue;
                }
            };

            if self.has_git_marker(&path, &entries) {
                self.add_repository(path);
                continue;
            }

            let mut child_directories = Vec::new();
            for entry in entries {
                let name = entry.file_name();
                if ignored_name(&name) {
                    continue;
                }
                let entry_path = entry.path();
                match entry.file_type() {
                    Ok(kind) if kind.is_symlink() => {
                        self.warning(DiscoveryWarningKind::SymlinkSkipped, entry_path);
                    }
                    Ok(kind) if kind.is_dir() => child_directories.push(entry_path),
                    Ok(_) => {}
                    Err(error) => self.warning(
                        DiscoveryWarningKind::EntryTypeFailed {
                            error_kind: error.kind(),
                        },
                        entry_path,
                    ),
                }
            }

            if depth >= self.options.max_depth {
                if !child_directories.is_empty() {
                    self.warning(
                        DiscoveryWarningKind::DepthLimitReached {
                            max_depth: self.options.max_depth,
                        },
                        path,
                    );
                }
                continue;
            }

            for child in child_directories.into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }

    fn finish(mut self) -> DiscoveryResult {
        self.repositories.sort();
        DiscoveryResult {
            repositories: self.repositories,
            warnings: self.warnings,
        }
    }
}

fn sorted_entries(path: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(DirEntry::file_name);
    Ok(entries)
}

fn ignored_name(name: &std::ffi::OsStr) -> bool {
    name == ".git"
        || name == ".cache"
        || name == "cache"
        || name == "node_modules"
        || name == "target"
}

/// Discover Git repositories beneath explicitly configured roots.
///
/// Root symlinks are canonicalized because the caller selected them explicitly.
/// Symlinked entries encountered below a root are never followed. Repositories
/// are recognized only by a real directory or regular-file `.git` marker.
pub fn discover_repositories(roots: &[PathBuf], options: DiscoveryOptions) -> DiscoveryResult {
    let mut state = DiscoveryState::new(options);
    let mut seen_roots = HashSet::new();

    for root in roots {
        let canonical_root = match fs::canonicalize(root) {
            Ok(path) => path,
            Err(error) => {
                state.warning(
                    DiscoveryWarningKind::RootUnavailable {
                        error_kind: error.kind(),
                    },
                    root.clone(),
                );
                continue;
            }
        };
        if !seen_roots.insert(canonical_root.clone()) {
            continue;
        }
        if state.repositories.len() >= options.max_repositories {
            state.repository_limit(&canonical_root);
            break;
        }
        state.walk_root(&canonical_root);
        if state.limit_reported {
            break;
        }
    }

    state.finish()
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
