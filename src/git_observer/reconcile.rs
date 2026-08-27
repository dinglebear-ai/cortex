//! One-repository Git reconciliation.

#[path = "reconcile_attribution.rs"]
mod attribution;
#[path = "reconcile_commits.rs"]
mod commit_import;
#[path = "reconcile_lifecycle.rs"]
mod lifecycle;
#[path = "reconcile_support.rs"]
mod support;
use attribution::attribute_commit_transitions;
use commit_import::{
    ObservedCommitTransition, collect_commit_changes, commit_upserts, transitions,
};
use lifecycle::{lifecycle_observations, previous_worktrees};
#[cfg(test)]
pub(crate) use support::GitCommandResult;
pub(crate) use support::{
    GitCommandRunner, ProcessGitRunner, ReconcileOptions, ReconcileStage, ReconcileWarning,
    ReconcileWarningKind, RepositoryReconcileReport,
};
use support::{
    canonical_or_prunable, count, display_name, optional_divergence, parse_branch_name,
    required_command, sha256_hex, strict_utf8, validate_snapshot_consistency, warning,
};

use crate::agent_observatory::identity::{repository_key, worktree_key};
use crate::db::DbPool;
use crate::db::agent_observatory::{
    GitRepositoryReconcileResult, RepositoryObservationInput, RepositoryObservationKind,
    RepositoryUpsert, RepositoryWorktreeUpsert, reconcile_git_repository_snapshot_with,
};
use crate::git_observer::porcelain::{
    StatusSummary, WorktreeRecord, parse_status_porcelain_v2, parse_worktree_porcelain,
};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct WorktreeSnapshot {
    upsert: RepositoryWorktreeUpsert,
    status_payload: String,
    head_payload: String,
}

struct RepositorySnapshot {
    repository: RepositoryUpsert,
    worktrees: Vec<WorktreeSnapshot>,
    discovered_payload: String,
}

async fn collect_worktree<R: GitCommandRunner>(
    runner: &mut R,
    record: WorktreeRecord,
    hostname: &str,
    timeout: Duration,
) -> std::result::Result<WorktreeSnapshot, ReconcileWarning> {
    let raw_path = strict_utf8(&record.path, ReconcileStage::WorktreePath, Path::new("."))?;
    let path = canonical_or_prunable(Path::new(&raw_path), record.prunable)?;
    let head = required_command(
        runner,
        &path,
        ReconcileStage::Head,
        &["rev-parse", "HEAD"],
        timeout,
    )
    .await?
    .trim()
    .to_string();
    let branch_output = required_command(
        runner,
        &path,
        ReconcileStage::Branch,
        &["rev-parse", "--abbrev-ref", "HEAD"],
        timeout,
    )
    .await?;
    let branch_name = parse_branch_name(&branch_output);

    let (status, status_raw) = if record.bare {
        (StatusSummary::default(), "bare".to_string())
    } else {
        let raw = required_command(
            runner,
            &path,
            ReconcileStage::Status,
            &["status", "--porcelain=v2", "--branch", "-z"],
            timeout,
        )
        .await?;
        let parsed = parse_status_porcelain_v2(raw.as_bytes()).map_err(|_| {
            warning(
                ReconcileStage::Status,
                &path,
                ReconcileWarningKind::ParseFailed,
            )
        })?;
        (parsed, raw)
    };

    if !record.bare {
        validate_snapshot_consistency(&record, &status, &head, branch_name.as_deref(), &path)?;
    }

    let upstream_ref = status
        .upstream
        .as_deref()
        .map(|value| strict_utf8(value, ReconcileStage::Status, &path))
        .transpose()?;
    let divergence = if upstream_ref.is_some() {
        optional_divergence(runner, &path, timeout).await?
    } else {
        None
    };
    if let Some((ahead, behind)) = divergence
        && (status
            .ahead
            .is_some_and(|value| i64::try_from(value).ok() != Some(ahead))
            || status
                .behind
                .is_some_and(|value| i64::try_from(value).ok() != Some(behind)))
    {
        return Err(warning(
            ReconcileStage::Divergence,
            &path,
            ReconcileWarningKind::SnapshotChanged,
        ));
    }
    let (ahead, behind) = divergence.unzip();
    let branch_ref = record
        .branch
        .as_deref()
        .map(|value| strict_utf8(value, ReconcileStage::Branch, &path))
        .transpose()?;
    let lock_reason = record
        .lock_reason
        .as_deref()
        .map(|value| strict_utf8(value, ReconcileStage::WorktreeList, &path))
        .transpose()?;
    let prune_reason = record
        .prune_reason
        .as_deref()
        .map(|value| strict_utf8(value, ReconcileStage::WorktreeList, &path))
        .transpose()?;
    let staged_count = count(status.staged_count, &path)?;
    let unstaged_count = count(status.unstaged_count, &path)?;
    let untracked_count = count(status.untracked_count, &path)?;
    let dirty = staged_count > 0
        || unstaged_count > 0
        || untracked_count > 0
        || status.conflicted_count > 0;
    let path_text = path
        .to_str()
        .ok_or_else(|| {
            warning(
                ReconcileStage::WorktreePath,
                &path,
                ReconcileWarningKind::InvalidUtf8,
            )
        })?
        .to_string();
    let git_dir = if record.bare {
        path_text.clone()
    } else {
        path.join(".git")
            .to_str()
            .ok_or_else(|| {
                warning(
                    ReconcileStage::WorktreePath,
                    &path,
                    ReconcileWarningKind::InvalidUtf8,
                )
            })?
            .to_string()
    };
    let status_hash = sha256_hex(status_raw.as_bytes());
    let key = worktree_key(hostname, &path_text).map_err(|_| {
        warning(
            ReconcileStage::WorktreePath,
            &path,
            ReconcileWarningKind::ParseFailed,
        )
    })?;
    let upsert = RepositoryWorktreeUpsert {
        worktree_key: key,
        hostname: hostname.to_string(),
        path: path_text,
        git_dir,
        branch_ref,
        branch_name,
        head_sha: Some(head.clone()),
        upstream_ref,
        detached: record.detached,
        bare: record.bare,
        locked: record.locked,
        lock_reason,
        prunable: record.prunable,
        prune_reason,
        dirty,
        staged_count,
        unstaged_count,
        untracked_count,
        ahead,
        behind,
        status_hash: Some(status_hash.clone()),
    };
    let status_payload = json!({
        "ahead": upsert.ahead,
        "bare": upsert.bare,
        "behind": upsert.behind,
        "branch_name": upsert.branch_name.as_deref(),
        "branch_ref": upsert.branch_ref.as_deref(),
        "detached": upsert.detached,
        "dirty": upsert.dirty,
        "locked": upsert.locked,
        "prunable": upsert.prunable,
        "staged_count": upsert.staged_count,
        "status_hash": status_hash,
        "unstaged_count": upsert.unstaged_count,
        "untracked_count": upsert.untracked_count,
        "upstream_ref": upsert.upstream_ref.as_deref(),
    })
    .to_string();
    let head_payload = json!({ "head_sha": head }).to_string();
    Ok(WorktreeSnapshot {
        upsert,
        status_payload,
        head_payload,
    })
}

async fn collect_repository<R: GitCommandRunner>(
    runner: &mut R,
    repository_path: &Path,
    options: &ReconcileOptions,
) -> std::result::Result<RepositorySnapshot, ReconcileWarning> {
    let common_raw = required_command(
        runner,
        repository_path,
        ReconcileStage::CommonDir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        options.command_timeout,
    )
    .await?;
    let common_path = fs::canonicalize(PathBuf::from(common_raw.trim())).map_err(|_| {
        warning(
            ReconcileStage::CommonDir,
            repository_path,
            ReconcileWarningKind::ExecutionFailed,
        )
    })?;
    let common_text = common_path
        .to_str()
        .ok_or_else(|| {
            warning(
                ReconcileStage::CommonDir,
                repository_path,
                ReconcileWarningKind::InvalidUtf8,
            )
        })?
        .to_string();
    let worktree_raw = required_command(
        runner,
        repository_path,
        ReconcileStage::WorktreeList,
        &["worktree", "list", "--porcelain", "-z"],
        options.command_timeout,
    )
    .await?;
    let records = parse_worktree_porcelain(worktree_raw.as_bytes()).map_err(|_| {
        warning(
            ReconcileStage::WorktreeList,
            repository_path,
            ReconcileWarningKind::ParseFailed,
        )
    })?;
    if records.is_empty() {
        return Err(warning(
            ReconcileStage::WorktreeList,
            repository_path,
            ReconcileWarningKind::ParseFailed,
        ));
    }
    let mut worktrees = Vec::with_capacity(records.len());
    for record in records {
        worktrees.push(
            collect_worktree(runner, record, &options.hostname, options.command_timeout).await?,
        );
    }
    worktrees.sort_by(|left, right| left.upsert.path.cmp(&right.upsert.path));
    let primary_text = repository_path
        .to_str()
        .ok_or_else(|| {
            warning(
                ReconcileStage::WorktreePath,
                repository_path,
                ReconcileWarningKind::InvalidUtf8,
            )
        })?
        .to_string();
    let key = repository_key(&options.hostname, &common_text).map_err(|_| {
        warning(
            ReconcileStage::CommonDir,
            repository_path,
            ReconcileWarningKind::ParseFailed,
        )
    })?;
    let repository = RepositoryUpsert {
        repository_key: key,
        hostname: options.hostname.clone(),
        common_git_dir: common_text.clone(),
        primary_path: primary_text.clone(),
        display_name: display_name(repository_path),
        remote_url_hash: None,
        metadata_json: json!({ "observer": "git", "version": 1 }).to_string(),
    };
    let discovered_payload = json!({
        "common_git_dir": common_text,
        "display_name": repository.display_name.as_str(),
        "primary_path": primary_text,
    })
    .to_string();
    Ok(RepositorySnapshot {
        repository,
        worktrees,
        discovered_payload,
    })
}

fn observations(
    snapshot: &RepositorySnapshot,
    transitions: &[ObservedCommitTransition],
) -> Vec<RepositoryObservationInput> {
    let mut inputs = Vec::with_capacity(1 + snapshot.worktrees.len() * 2);
    inputs.push(RepositoryObservationInput {
        worktree_key: None,
        observation_kind: RepositoryObservationKind::Discovered,
        new_head_sha: None,
        summary: "repository discovered".to_string(),
        payload_json: snapshot.discovered_payload.clone(),
    });
    for worktree in &snapshot.worktrees {
        inputs.push(RepositoryObservationInput {
            worktree_key: Some(worktree.upsert.worktree_key.clone()),
            observation_kind: RepositoryObservationKind::Status,
            new_head_sha: None,
            summary: "worktree status changed".to_string(),
            payload_json: worktree.status_payload.clone(),
        });
        let transition = transitions
            .iter()
            .find(|transition| transition.path == Path::new(&worktree.upsert.path));
        let (summary, payload_json) = transition.map_or_else(
            || {
                (
                    "worktree HEAD changed".to_string(),
                    worktree.head_payload.clone(),
                )
            },
            |transition| {
                let summary = if transition.kind.is_fast_forward() {
                    "worktree HEAD changed"
                } else {
                    "worktree HEAD changed (non-fast-forward)"
                };
                (
                    summary.to_string(),
                    json!({
                        "detached": worktree.upsert.detached,
                        "displaced_commit_count": transition.displaced_commit_count,
                        "fast_forward": transition.kind.is_fast_forward(),
                        "head_sha": transition.new_sha.as_str(),
                        "new_commit_count": transition.new_commit_count,
                        "new_commit_shas": transition.new_shas,
                        "displaced_commit_shas": transition.displaced_shas,
                        "old_head_sha": transition.old_sha.as_str(),
                        "transition_kind": transition.kind.as_str(),
                    })
                    .to_string(),
                )
            },
        );
        inputs.push(RepositoryObservationInput {
            worktree_key: Some(worktree.upsert.worktree_key.clone()),
            observation_kind: RepositoryObservationKind::Head,
            new_head_sha: worktree.upsert.head_sha.clone(),
            summary,
            payload_json,
        });
    }
    inputs
}

pub(crate) async fn reconcile_one_repository(
    pool: &Arc<DbPool>,
    repository_path: &Path,
    options: &ReconcileOptions,
    observed_at: &str,
) -> Result<RepositoryReconcileReport> {
    let mut runner = ProcessGitRunner;
    reconcile_one_repository_with_runner(pool, repository_path, options, observed_at, &mut runner)
        .await
}

/// Reconcile one repository: collect a Git snapshot, then persist it.
///
/// The Git half is genuinely async (subprocesses). The database half is
/// blocking — `previous_worktrees` reads, and the reconcile plus its commit
/// attribution take a pooled connection and the process-wide write lock — so it
/// runs on `spawn_blocking` rather than on the async task, which would otherwise
/// park a runtime worker for the whole write-lock wait. Same treatment the
/// Agent Observatory projector got, and `docker_ingest::supervisor` before it
/// (full-review PM8); the pool takes `Arc<DbPool>` for the same reason.
///
/// The reconcile and the attribution share one blocking hop: attribution reads
/// the reconcile's own result, so splitting them would buy a second pool
/// acquisition and nothing else.
pub(crate) async fn reconcile_one_repository_with_runner<R: GitCommandRunner>(
    pool: &Arc<DbPool>,
    repository_path: &Path,
    options: &ReconcileOptions,
    observed_at: &str,
    runner: &mut R,
) -> Result<RepositoryReconcileReport> {
    if options.hostname.trim().is_empty() {
        bail!("hostname must be non-empty");
    }
    if options.command_timeout.is_zero() {
        bail!("command_timeout must be positive");
    }
    if options.max_commits_per_transition == 0 {
        bail!("max_commits_per_transition must be positive");
    }
    chrono::DateTime::parse_from_rfc3339(observed_at)
        .with_context(|| format!("invalid observed_at: {observed_at}"))?;
    let repository_path = fs::canonicalize(repository_path)
        .context("repository path must exist and be canonicalizable")?;
    let snapshot = match collect_repository(runner, &repository_path, options).await {
        Ok(snapshot) => snapshot,
        Err(warning) => return Ok(RepositoryReconcileReport::warning(warning)),
    };
    let (repository_existed, previous) = {
        let pool = Arc::clone(pool);
        let repository_key = snapshot.repository.repository_key.clone();
        tokio::task::spawn_blocking(move || previous_worktrees(&pool, &repository_key))
            .await
            .context("previous-worktree lookup task failed")??
    };
    let worktrees = snapshot
        .worktrees
        .iter()
        .map(|worktree| worktree.upsert.clone())
        .collect::<Vec<_>>();
    let commit_transitions = transitions(&previous, &worktrees);
    let commit_collection =
        match collect_commit_changes(runner, &commit_transitions, &worktrees, options).await {
            Ok(collection) => collection,
            Err(warning) => return Ok(RepositoryReconcileReport::warning(warning)),
        };
    let commit_inputs =
        commit_upserts(&commit_collection.commits, &commit_collection.reachability)?;
    let base_observations = observations(&snapshot, &commit_collection.transitions);
    let result = {
        let pool = Arc::clone(pool);
        let repository = snapshot.repository;
        let observed_at = observed_at.to_string();
        let lifecycle_observed_at = observed_at.clone();
        let reachability = commit_collection.reachability;
        let observed_transitions = commit_collection.transitions;
        tokio::task::spawn_blocking(move || -> Result<GitRepositoryReconcileResult> {
            let result = reconcile_git_repository_snapshot_with(
                &pool,
                &repository,
                &worktrees,
                &commit_inputs,
                &reachability,
                &observed_at,
                |topology| {
                    let mut inputs = base_observations;
                    inputs.extend(lifecycle_observations(
                        repository_existed,
                        &previous,
                        topology,
                        &lifecycle_observed_at,
                    )?);
                    Ok(inputs)
                },
            )?;
            attribute_commit_transitions(&pool, &observed_transitions, &result);
            Ok(result)
        })
        .await
        .context("git repository reconcile task failed")??
    };
    Ok(RepositoryReconcileReport {
        topology: Some(result.topology),
        imported_commits: result.commits,
        inserted_observations: result.observations,
        warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "reconcile_commits_tests.rs"]
mod commit_tests;

#[cfg(test)]
#[path = "reconcile_rewrites_tests.rs"]
mod rewrite_tests;

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
