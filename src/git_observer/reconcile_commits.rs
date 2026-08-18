//! Bounded fast-forward commit planning for repository reconciliation.

use super::{
    GitCommandRunner, ReconcileOptions, ReconcileStage, ReconcileWarning, ReconcileWarningKind,
};
use crate::db::agent_observatory::{
    GitCommitReachabilityUpdate, GitCommitUpsert, RepositoryWorktreeRow, RepositoryWorktreeUpsert,
};
use crate::git_observer::commits::{
    CommitParseOptions, ParsedCommit, commit_show_arguments, parse_commit_show,
};
use crate::inventory::limits::MAX_COMMAND_OUTPUT_BYTES;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommitTransition {
    pub path: PathBuf,
    pub old_sha: String,
    pub new_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommitTransitionKind {
    FastForward,
    Rewind,
    Rewrite,
}

impl CommitTransitionKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::FastForward => "fast_forward",
            Self::Rewind => "rewind",
            Self::Rewrite => "rewrite",
        }
    }

    pub(super) const fn is_fast_forward(self) -> bool {
        matches!(self, Self::FastForward)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObservedCommitTransition {
    pub path: PathBuf,
    pub old_sha: String,
    pub new_sha: String,
    pub kind: CommitTransitionKind,
    pub new_shas: Vec<String>,
    pub displaced_shas: Vec<String>,
    pub new_commit_count: usize,
    pub displaced_commit_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct CommitCollection {
    pub commits: Vec<ParsedCommit>,
    pub reachability: Vec<GitCommitReachabilityUpdate>,
    pub transitions: Vec<ObservedCommitTransition>,
}

fn warning(stage: ReconcileStage, path: &Path, kind: ReconcileWarningKind) -> ReconcileWarning {
    ReconcileWarning {
        stage,
        path: path.to_path_buf(),
        kind,
    }
}

fn git_args(path: &Path, command: &[String]) -> Result<Vec<String>, ReconcileWarning> {
    let Some(path) = path.to_str() else {
        return Err(warning(
            ReconcileStage::CommitTraversal,
            path,
            ReconcileWarningKind::InvalidUtf8,
        ));
    };
    let mut args = vec!["-C".to_string(), path.to_string()];
    args.extend(command.iter().cloned());
    Ok(args)
}

fn valid_object_id(value: &[u8]) -> bool {
    matches!(value.len(), 40 | 64) && value.iter().all(u8::is_ascii_hexdigit)
}

async fn is_ancestor<R: GitCommandRunner>(
    runner: &mut R,
    path: &Path,
    ancestor: &str,
    descendant: &str,
    timeout: std::time::Duration,
) -> Result<bool, ReconcileWarning> {
    let command = vec![
        "merge-base".to_string(),
        "--is-ancestor".to_string(),
        ancestor.to_string(),
        descendant.to_string(),
    ];
    let output = runner
        .run(git_args(path, &command)?, timeout)
        .await
        .map_err(|_| {
            warning(
                ReconcileStage::CommitTraversal,
                path,
                ReconcileWarningKind::ExecutionFailed,
            )
        })?;
    if output.truncated {
        return Err(warning(
            ReconcileStage::CommitTraversal,
            path,
            ReconcileWarningKind::OutputTruncated,
        ));
    }
    match output.status {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        status => Err(warning(
            ReconcileStage::CommitTraversal,
            path,
            ReconcileWarningKind::CommandFailed { status },
        )),
    }
}

async fn rev_list<R: GitCommandRunner>(
    runner: &mut R,
    path: &Path,
    exclude: &str,
    include: &str,
    options: &ReconcileOptions,
) -> Result<Vec<String>, ReconcileWarning> {
    let probe_limit = options
        .max_commits_per_transition
        .checked_add(1)
        .ok_or_else(|| {
            warning(
                ReconcileStage::CommitTraversal,
                path,
                ReconcileWarningKind::CommitLimitReached {
                    limit: options.max_commits_per_transition,
                },
            )
        })?;
    let command = vec![
        "rev-list".to_string(),
        "--reverse".to_string(),
        format!("--max-count={probe_limit}"),
        format!("{exclude}..{include}"),
    ];
    let output = runner
        .run(git_args(path, &command)?, options.command_timeout)
        .await
        .map_err(|_| {
            warning(
                ReconcileStage::CommitTraversal,
                path,
                ReconcileWarningKind::ExecutionFailed,
            )
        })?;
    if output.truncated {
        return Err(warning(
            ReconcileStage::CommitTraversal,
            path,
            ReconcileWarningKind::OutputTruncated,
        ));
    }
    if output.status != Some(0) {
        return Err(warning(
            ReconcileStage::CommitTraversal,
            path,
            ReconcileWarningKind::CommandFailed {
                status: output.status,
            },
        ));
    }

    let mut shas = Vec::new();
    for line in output.stdout_bytes.split(|byte| *byte == 10) {
        if line.is_empty() {
            continue;
        }
        if !valid_object_id(line) {
            return Err(warning(
                ReconcileStage::CommitTraversal,
                path,
                ReconcileWarningKind::ParseFailed,
            ));
        }
        shas.push(String::from_utf8(line.to_vec()).expect("object ID is ASCII"));
    }
    if shas.len() > options.max_commits_per_transition {
        return Err(warning(
            ReconcileStage::CommitTraversal,
            path,
            ReconcileWarningKind::CommitLimitReached {
                limit: options.max_commits_per_transition,
            },
        ));
    }
    Ok(shas)
}

async fn commit_metadata<R: GitCommandRunner>(
    runner: &mut R,
    path: &Path,
    shas: &[String],
    options: &ReconcileOptions,
) -> Result<Vec<ParsedCommit>, ReconcileWarning> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let command =
        commit_show_arguments(shas, options.max_commits_per_transition).map_err(|_| {
            warning(
                ReconcileStage::CommitMetadata,
                path,
                ReconcileWarningKind::ParseFailed,
            )
        })?;
    let output = runner
        .run(git_args(path, &command)?, options.command_timeout)
        .await
        .map_err(|_| {
            warning(
                ReconcileStage::CommitMetadata,
                path,
                ReconcileWarningKind::ExecutionFailed,
            )
        })?;
    if output.truncated {
        return Err(warning(
            ReconcileStage::CommitMetadata,
            path,
            ReconcileWarningKind::OutputTruncated,
        ));
    }
    if output.status != Some(0) {
        return Err(warning(
            ReconcileStage::CommitMetadata,
            path,
            ReconcileWarningKind::CommandFailed {
                status: output.status,
            },
        ));
    }
    let parsed = parse_commit_show(
        &output.stdout_bytes,
        CommitParseOptions {
            max_input_bytes: MAX_COMMAND_OUTPUT_BYTES,
            max_commits: options.max_commits_per_transition,
            max_paths_per_commit: 2_000,
            store_changed_paths: options.store_changed_paths,
            store_author_name: options.store_author_name,
            store_author_email_hash: options.store_author_email_hash,
        },
    )
    .map_err(|_| {
        warning(
            ReconcileStage::CommitMetadata,
            path,
            ReconcileWarningKind::ParseFailed,
        )
    })?;
    if parsed.len() != shas.len()
        || parsed
            .iter()
            .zip(shas)
            .any(|(commit, expected)| commit.sha != *expected)
    {
        return Err(warning(
            ReconcileStage::CommitMetadata,
            path,
            ReconcileWarningKind::ParseFailed,
        ));
    }
    Ok(parsed)
}

pub(super) fn transitions(
    previous: &[RepositoryWorktreeRow],
    current: &[RepositoryWorktreeUpsert],
) -> Vec<CommitTransition> {
    let previous = previous
        .iter()
        .map(|row| (row.worktree_key.as_str(), row))
        .collect::<HashMap<_, _>>();
    current
        .iter()
        .filter_map(|worktree| {
            let old = previous.get(worktree.worktree_key.as_str())?;
            if old.removed_at.is_some() {
                return None;
            }
            let old_sha = old.head_sha.as_ref()?;
            let new_sha = worktree.head_sha.as_ref()?;
            (old_sha != new_sha).then(|| CommitTransition {
                path: PathBuf::from(&worktree.path),
                old_sha: old_sha.clone(),
                new_sha: new_sha.clone(),
            })
        })
        .collect()
}

fn current_heads(current: &[RepositoryWorktreeUpsert]) -> Vec<String> {
    let mut seen = HashSet::new();
    current
        .iter()
        .filter_map(|worktree| worktree.head_sha.clone())
        .filter(|sha| seen.insert(sha.clone()))
        .collect()
}

async fn reachable_from_heads<R: GitCommandRunner>(
    runner: &mut R,
    path: &Path,
    sha: &str,
    heads: &[String],
    timeout: std::time::Duration,
) -> Result<bool, ReconcileWarning> {
    for head in heads {
        if sha == head || is_ancestor(runner, path, sha, head, timeout).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn merge_reachability(updates: &mut Vec<GitCommitReachabilityUpdate>, sha: &str, reachable: bool) {
    if let Some(existing) = updates.iter_mut().find(|update| update.sha == sha) {
        existing.reachable |= reachable;
    } else {
        updates.push(GitCommitReachabilityUpdate {
            sha: sha.to_string(),
            reachable,
        });
    }
}

pub(super) async fn collect_commit_changes<R: GitCommandRunner>(
    runner: &mut R,
    transitions: &[CommitTransition],
    current: &[RepositoryWorktreeUpsert],
    options: &ReconcileOptions,
) -> Result<CommitCollection, ReconcileWarning> {
    let heads = current_heads(current);
    let mut collection = CommitCollection::default();
    let mut seen = HashSet::new();

    for transition in transitions {
        let fast_forward = is_ancestor(
            runner,
            &transition.path,
            &transition.old_sha,
            &transition.new_sha,
            options.command_timeout,
        )
        .await?;
        let kind = if fast_forward {
            CommitTransitionKind::FastForward
        } else if is_ancestor(
            runner,
            &transition.path,
            &transition.new_sha,
            &transition.old_sha,
            options.command_timeout,
        )
        .await?
        {
            CommitTransitionKind::Rewind
        } else {
            CommitTransitionKind::Rewrite
        };

        let new_shas = rev_list(
            runner,
            &transition.path,
            &transition.old_sha,
            &transition.new_sha,
            options,
        )
        .await?;
        let displaced_shas = if kind.is_fast_forward() {
            Vec::new()
        } else {
            rev_list(
                runner,
                &transition.path,
                &transition.new_sha,
                &transition.old_sha,
                options,
            )
            .await?
        };
        let mut metadata_shas = new_shas.clone();
        metadata_shas.extend(displaced_shas.iter().cloned());
        for commit in commit_metadata(runner, &transition.path, &metadata_shas, options).await? {
            if seen.insert(commit.sha.clone()) {
                collection.commits.push(commit);
            }
        }
        for sha in &new_shas {
            merge_reachability(&mut collection.reachability, sha, true);
        }
        for sha in &displaced_shas {
            let reachable = reachable_from_heads(
                runner,
                &transition.path,
                sha,
                &heads,
                options.command_timeout,
            )
            .await?;
            merge_reachability(&mut collection.reachability, sha, reachable);
        }
        collection.transitions.push(ObservedCommitTransition {
            path: transition.path.clone(),
            old_sha: transition.old_sha.clone(),
            new_sha: transition.new_sha.clone(),
            kind,
            new_commit_count: new_shas.len(),
            displaced_commit_count: displaced_shas.len(),
            new_shas,
            displaced_shas,
        });
    }
    Ok(collection)
}

fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn count(value: u64, field: &str) -> anyhow::Result<i64> {
    i64::try_from(value).map_err(|_| anyhow::anyhow!("{field} exceeds SQLite integer range"))
}

pub(super) fn commit_upserts(
    commits: &[ParsedCommit],
    reachability: &[GitCommitReachabilityUpdate],
) -> anyhow::Result<Vec<GitCommitUpsert>> {
    let reachability = reachability
        .iter()
        .map(|update| (update.sha.as_str(), update.reachable))
        .collect::<HashMap<_, _>>();
    commits
        .iter()
        .map(|commit| {
            let changed_paths = commit
                .changed_paths
                .iter()
                .map(|change| {
                    json!({
                        "binary": change.binary,
                        "deletions": change.deletions,
                        "insertions": change.insertions,
                        "path_hex": hex(&change.path),
                        "previous_path_hex": change.previous_path.as_deref().map(hex),
                    })
                })
                .collect::<Vec<_>>();
            Ok(GitCommitUpsert {
                sha: commit.sha.clone(),
                parent_shas_json: serde_json::to_string(&commit.parent_shas)?,
                author_name: commit.author_name.clone(),
                author_email_hash: commit.author_email_hash.clone(),
                authored_at: Some(commit.authored_at.clone()),
                committed_at: Some(commit.committed_at.clone()),
                subject: commit.subject.clone(),
                changed_files: Some(count(commit.changed_files, "changed_files")?),
                insertions: Some(count(commit.insertions, "insertions")?),
                deletions: Some(count(commit.deletions, "deletions")?),
                changed_paths_json: serde_json::to_string(&changed_paths)?,
                reachable: reachability
                    .get(commit.sha.as_str())
                    .copied()
                    .unwrap_or(true),
                metadata_json: json!({
                    "binary_files": commit.binary_files,
                    "path_encoding": "hex",
                    "paths_truncated": commit.paths_truncated,
                })
                .to_string(),
            })
        })
        .collect()
}
