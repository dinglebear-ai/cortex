//! Command and validation support for one-repository reconciliation.

use crate::db::agent_observatory::{
    GitCommitRow, RepositoryObservationRow, RepositoryReconcileResult,
};
use crate::git_observer::porcelain::{StatusSummary, WorktreeRecord};
use crate::inventory::limits::MAX_COMMAND_OUTPUT_BYTES;
use crate::inventory::process::run_command_bytes_capped;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconcileOptions {
    pub hostname: String,
    pub command_timeout: Duration,
    pub max_commits_per_transition: usize,
    pub store_changed_paths: bool,
    pub store_author_name: bool,
    pub store_author_email_hash: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReconcileStage {
    CommonDir,
    WorktreeList,
    Status,
    Head,
    Branch,
    Divergence,
    CommitTraversal,
    CommitMetadata,
    WorktreePath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReconcileWarningKind {
    ExecutionFailed,
    CommandFailed { status: Option<i32> },
    OutputTruncated,
    ParseFailed,
    InvalidUtf8,
    SnapshotChanged,
    CommitLimitReached { limit: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconcileWarning {
    pub stage: ReconcileStage,
    pub path: PathBuf,
    pub kind: ReconcileWarningKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepositoryReconcileReport {
    pub topology: Option<RepositoryReconcileResult>,
    pub imported_commits: Vec<GitCommitRow>,
    pub inserted_observations: Vec<RepositoryObservationRow>,
    pub warnings: Vec<ReconcileWarning>,
}

impl RepositoryReconcileReport {
    pub(super) fn warning(warning: ReconcileWarning) -> Self {
        Self {
            topology: None,
            imported_commits: Vec::new(),
            inserted_observations: Vec::new(),
            warnings: vec![warning],
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GitCommandResult {
    pub(super) status: Option<i32>,
    pub(super) stdout: String,
    pub(super) stdout_bytes: Vec<u8>,
    pub(super) truncated: bool,
}

#[allow(async_fn_in_trait)]
pub(crate) trait GitCommandRunner {
    async fn run(&mut self, args: Vec<String>, timeout: Duration) -> Result<GitCommandResult>;
}

#[derive(Debug, Default)]
pub(crate) struct ProcessGitRunner;

impl GitCommandRunner for ProcessGitRunner {
    async fn run(&mut self, args: Vec<String>, timeout: Duration) -> Result<GitCommandResult> {
        let references = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output =
            run_command_bytes_capped("git", &references, timeout, MAX_COMMAND_OUTPUT_BYTES).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(GitCommandResult {
            status: output.status,
            stdout,
            stdout_bytes: output.stdout,
            truncated: output.truncated,
        })
    }
}

pub(super) fn warning(
    stage: ReconcileStage,
    path: &Path,
    kind: ReconcileWarningKind,
) -> ReconcileWarning {
    ReconcileWarning {
        stage,
        path: path.to_path_buf(),
        kind,
    }
}

fn git_args(path: &Path, command: &[&str]) -> Result<Vec<String>> {
    let path = path
        .to_str()
        .context("Git reconciliation path must be valid UTF-8")?;
    let mut args = vec!["-C".to_string(), path.to_string()];
    args.extend(command.iter().map(|argument| (*argument).to_string()));
    Ok(args)
}

pub(super) async fn required_command<R: GitCommandRunner>(
    runner: &mut R,
    path: &Path,
    stage: ReconcileStage,
    command: &[&str],
    timeout: Duration,
) -> std::result::Result<String, ReconcileWarning> {
    let args = git_args(path, command)
        .map_err(|_| warning(stage, path, ReconcileWarningKind::InvalidUtf8))?;
    let output = runner
        .run(args, timeout)
        .await
        .map_err(|_| warning(stage, path, ReconcileWarningKind::ExecutionFailed))?;
    if output.truncated {
        return Err(warning(stage, path, ReconcileWarningKind::OutputTruncated));
    }
    if output.status != Some(0) {
        return Err(warning(
            stage,
            path,
            ReconcileWarningKind::CommandFailed {
                status: output.status,
            },
        ));
    }
    Ok(output.stdout)
}

pub(super) async fn optional_divergence<R: GitCommandRunner>(
    runner: &mut R,
    path: &Path,
    timeout: Duration,
) -> std::result::Result<Option<(i64, i64)>, ReconcileWarning> {
    let args = git_args(
        path,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )
    .map_err(|_| {
        warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::InvalidUtf8,
        )
    })?;
    let output = runner.run(args, timeout).await.map_err(|_| {
        warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::ExecutionFailed,
        )
    })?;
    if output.truncated {
        return Err(warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::OutputTruncated,
        ));
    }
    if output.status != Some(0) {
        return Ok(None);
    }
    let values = output.stdout.split_whitespace().collect::<Vec<_>>();
    if values.len() != 2 {
        return Err(warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::ParseFailed,
        ));
    }
    let behind = values[0].parse::<i64>().map_err(|_| {
        warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::ParseFailed,
        )
    })?;
    let ahead = values[1].parse::<i64>().map_err(|_| {
        warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::ParseFailed,
        )
    })?;
    if ahead < 0 || behind < 0 {
        return Err(warning(
            ReconcileStage::Divergence,
            path,
            ReconcileWarningKind::ParseFailed,
        ));
    }
    Ok(Some((ahead, behind)))
}

pub(super) fn strict_utf8(
    bytes: &[u8],
    stage: ReconcileStage,
    path: &Path,
) -> std::result::Result<String, ReconcileWarning> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| warning(stage, path, ReconcileWarningKind::InvalidUtf8))
}

pub(super) fn canonical_or_prunable(
    path: &Path,
    prunable: bool,
) -> std::result::Result<PathBuf, ReconcileWarning> {
    if let Ok(canonical) = fs::canonicalize(path) {
        return Ok(canonical);
    }
    let is_clean_absolute = path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir));
    if prunable && is_clean_absolute {
        return Ok(path.to_path_buf());
    }
    Err(warning(
        ReconcileStage::WorktreePath,
        path,
        ReconcileWarningKind::ExecutionFailed,
    ))
}

pub(super) fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(super) fn parse_branch_name(value: &str) -> Option<String> {
    let value = value.trim();
    (value != "HEAD" && !value.is_empty()).then(|| value.to_string())
}

pub(super) fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("repository")
        .to_string()
}

pub(super) fn count(value: u64, path: &Path) -> std::result::Result<i64, ReconcileWarning> {
    i64::try_from(value).map_err(|_| {
        warning(
            ReconcileStage::Status,
            path,
            ReconcileWarningKind::ParseFailed,
        )
    })
}

pub(super) fn validate_snapshot_consistency(
    record: &WorktreeRecord,
    status: &StatusSummary,
    head: &str,
    branch_name: Option<&str>,
    path: &Path,
) -> std::result::Result<(), ReconcileWarning> {
    if record.head.as_deref().is_some_and(|value| value != head)
        || status
            .branch_oid
            .as_deref()
            .is_some_and(|value| value != head)
    {
        return Err(warning(
            ReconcileStage::Head,
            path,
            ReconcileWarningKind::SnapshotChanged,
        ));
    }
    if status.detached != record.detached || status.detached != branch_name.is_none() {
        return Err(warning(
            ReconcileStage::Branch,
            path,
            ReconcileWarningKind::SnapshotChanged,
        ));
    }
    if status
        .branch_head
        .as_deref()
        .and_then(|value| std::str::from_utf8(value).ok())
        .zip(branch_name)
        .is_some_and(|(status_branch, branch)| status_branch != branch)
    {
        return Err(warning(
            ReconcileStage::Branch,
            path,
            ReconcileWarningKind::SnapshotChanged,
        ));
    }
    Ok(())
}
