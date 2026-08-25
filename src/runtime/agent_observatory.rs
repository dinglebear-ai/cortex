//! Bounded, cancellable Agent Observatory projector and Git reconcile workers.

use crate::agent_observatory::projector::{
    project_agent_source_with_cursor, project_log_row_with_cursor,
};
use crate::config::AgentObservatoryConfig;
use crate::db::agent_observatory::{
    AgentSourceKind, advance_projection_cursor, page_agent_sources, projection_cursor,
    projection_wake_receiver, record_projection_health,
};
use crate::db::page_agent_projection_logs;
use crate::db::{DbPool, TRANSIENT_SQLITE_RETRY_DELAYS_MS, is_transient_sqlite_lock};
use crate::git_observer::discovery::{DiscoveryOptions, discover_repositories};
use crate::git_observer::reconcile::{ReconcileOptions, reconcile_one_repository};
use crate::scanner::local_hostname;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const SOURCE_KINDS: [AgentSourceKind; 4] = [
    AgentSourceKind::Mcp,
    AgentSourceKind::Hook,
    AgentSourceKind::Skill,
    AgentSourceKind::Llm,
];

fn source_name(kind: AgentSourceKind) -> &'static str {
    kind.as_str()
}

fn projector_retry_delay_ms(attempt: usize) -> u64 {
    let index = attempt
        .saturating_sub(1)
        .min(TRANSIENT_SQLITE_RETRY_DELAYS_MS.len() - 1);
    TRANSIENT_SQLITE_RETRY_DELAYS_MS[index]
}

/// Cumulative, monotonic progress of one projector instance.
///
/// A projector cycle has no wall-clock bound: it takes the process-wide SQLite
/// write serialization lock (`db::pool::write_lock`) several times, so under
/// contention — most visibly the parallel test suite, where every test's pool
/// queues on that one lock — a single cycle can take tens of seconds. Anything
/// that needs to know a cycle happened must therefore observe this signal
/// rather than assume a cycle fits inside some interval.
///
/// Every field is cumulative and monotonic, so an observer can never miss a
/// transition by sampling late — unlike the health row, whose `detail` describes
/// only the cycle that wrote it and is overwritten by the next one.
/// `health_records` counts only successfully persisted health rows, so it
/// tracks the `attempts` counter inside the stored health JSON exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ProjectorProgress {
    pub(super) cycles: u64,
    pub(super) health_records: u64,
    pub(super) projected: u64,
    pub(super) oversized_first_rows: u64,
}

/// Spawn the projector, returning its join handle plus a receiver that reports
/// [`ProjectorProgress`] after every completed cycle. Production drops the
/// receiver; `watch::Sender::send_replace` never fails on a dropped receiver,
/// so the signal costs one `watch` cell whether or not anyone is listening.
pub(super) fn spawn_projector(
    token: CancellationToken,
    pool: Arc<DbPool>,
    config: AgentObservatoryConfig,
) -> Option<(JoinHandle<()>, watch::Receiver<ProjectorProgress>)> {
    config.enabled.then(|| {
        let mut wake = projection_wake_receiver();
        let (progress_tx, progress_rx) = watch::channel(ProjectorProgress::default());
        let task = tokio::spawn(async move {
            let mut progress = ProjectorProgress::default();
            let mut consecutive_retry_safe_failures = 0usize;
            loop {
                if token.is_cancelled() { break; }
                let mut projected = 0usize;
                let mut healthy = true;
                let mut had_error = false;
                let mut retry_safe_errors_only = true;
                let mut oversized_first_rows = 0usize;
                let log_page = projection_cursor(&pool, "logs")
                    .and_then(|value| {
                        if value.is_empty() {
                            Ok(0)
                        } else {
                            value.parse::<i64>().map_err(anyhow::Error::from)
                        }
                    })
                    .and_then(|cursor| {
                        page_agent_projection_logs(&pool, cursor, config.projector_page_rows)
                    });
                match log_page {
                    Ok(rows) => {
                        let mut page_bytes = 0usize;
                        for (processed, row) in rows.into_iter().enumerate() {
                            page_bytes = page_bytes.saturating_add(row.message.len());
                            if page_bytes > config.projector_page_bytes && processed > 0 { break; }
                            oversized_first_rows += usize::from(
                                page_bytes > config.projector_page_bytes && processed == 0,
                            );
                            match project_log_row_with_cursor(&pool, &row) {
                                Ok(()) => projected += 1,
                                Err(error) => {
                                    healthy = false;
                                    had_error = true;
                                    retry_safe_errors_only &= is_transient_sqlite_lock(&error);
                                    tracing::error!(error = %error,
                                        "Agent Observatory log projection failed");
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        healthy = false;
                        had_error = true;
                        retry_safe_errors_only &= is_transient_sqlite_lock(&error);
                        tracing::error!(error = %error, "Agent Observatory log page failed");
                    }
                }
                for kind in SOURCE_KINDS {
                    let name = source_name(kind);
                    let cursor = match projection_cursor(&pool, name) {
                        Ok(cursor) => cursor,
                        Err(error) => {
                            healthy = false;
                            had_error = true;
                            retry_safe_errors_only &= is_transient_sqlite_lock(&error);
                            tracing::error!(source_kind = ?kind, error = %error,
                                "Agent Observatory source cursor load failed");
                            continue;
                        }
                    };
                    match page_agent_sources(&pool, kind, &cursor, config.projector_page_rows) {
                        Ok(page) => {
                            let mut page_bytes = 0usize;
                            for (processed, record) in page.records.iter().enumerate() {
                                page_bytes = page_bytes.saturating_add(format!("{record:?}").len());
                                if page_bytes > config.projector_page_bytes && processed > 0 {
                                    break;
                                }
                                oversized_first_rows += usize::from(
                                    page_bytes > config.projector_page_bytes && processed == 0,
                                );
                                let next_cursor = record.next_cursor();
                                match project_agent_source_with_cursor(
                                    &pool,
                                    record,
                                    name,
                                    &next_cursor,
                                ) {
                                    Ok(_) => projected += 1,
                                    Err(error) => {
                                        healthy = false;
                                        had_error = true;
                                        retry_safe_errors_only &= is_transient_sqlite_lock(&error);
                                        tracing::error!(
                                        source_kind = ?kind,
                                        error = %error,
                                        "Agent Observatory source projection failed"
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            healthy = false;
                            had_error = true;
                            retry_safe_errors_only &= is_transient_sqlite_lock(&error);
                            tracing::error!(source_kind = ?kind, error = %error,
                                "Agent Observatory source page failed");
                        }
                    }
                }
                let retry_safe_failure = had_error && retry_safe_errors_only;
                let retry_delay_ms = if retry_safe_failure {
                    consecutive_retry_safe_failures =
                        consecutive_retry_safe_failures.saturating_add(1);
                    Some(projector_retry_delay_ms(consecutive_retry_safe_failures))
                } else {
                    consecutive_retry_safe_failures = 0;
                    None
                };
                let status = if healthy { "ok" } else { "error" };
                match record_projection_health(
                    &pool,
                    "projector",
                    status,
                    &format!(
                        "projected={projected},oversized_first_rows={oversized_first_rows},retry_safe={retry_safe_failure},retry_delay_ms={}",
                        retry_delay_ms.unwrap_or(0)
                    ),
                ) {
                    Ok(()) => progress.health_records = progress.health_records.saturating_add(1),
                    Err(error) => {
                        tracing::error!(error = %error, "Agent Observatory projector health write failed");
                    }
                }
                tracing::debug!(projected, "Agent Observatory projector cycle completed");
                progress.cycles = progress.cycles.saturating_add(1);
                progress.projected = progress.projected.saturating_add(projected as u64);
                progress.oversized_first_rows = progress
                    .oversized_first_rows
                    .saturating_add(oversized_first_rows as u64);
                // Published after the health row is committed so an observer
                // woken by this signal always reads the row this cycle wrote.
                progress_tx.send_replace(progress);
                if healthy {
                    tokio::select! {
                        biased;
                        () = token.cancelled() => break,
                        _ = wake.recv() => {},
                        () = tokio::time::sleep(Duration::from_millis(config.projector_poll_ms)) => {}
                    }
                } else {
                    let delay_ms = retry_delay_ms.unwrap_or(config.projector_poll_ms);
                    tokio::select! {
                        biased;
                        () = token.cancelled() => break,
                        () = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                    }
                }
            }
        });
        (task, progress_rx)
    })
}

fn expand_root(root: &str) -> PathBuf {
    root.strip_prefix("~/").map_or_else(
        || PathBuf::from(root),
        |suffix| {
            crate::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/nonexistent"))
                .join(suffix)
        },
    )
}

pub(super) fn spawn_git_reconcile(
    token: CancellationToken,
    pool: Arc<DbPool>,
    config: AgentObservatoryConfig,
) -> Option<JoinHandle<()>> {
    (config.enabled && config.git.enabled).then(|| {
        tokio::spawn(async move {
            let roots = config.git.roots.iter().map(|root| expand_root(root)).collect::<Vec<_>>();
            let discovery_options = DiscoveryOptions {
                max_depth: config.git.max_depth,
                max_repositories: config.git.max_repositories,
            };
            let options = ReconcileOptions {
                hostname: local_hostname(),
                command_timeout: Duration::from_millis(config.git.command_timeout_ms),
                max_commits_per_transition: config.git.max_commits_per_transition,
                store_changed_paths: config.git.store_changed_paths,
                store_author_name: config.git.store_author_name,
                store_author_email_hash: config.git.store_author_email_hash,
            };
            loop {
                if token.is_cancelled() { break; }
                let discovery = discover_repositories(&roots, discovery_options);
                let mut healthy = discovery.warnings.is_empty();
                for warning in discovery.warnings {
                    tracing::warn!(path = %warning.path.display(), kind = ?warning.kind,
                        "Agent Observatory Git discovery warning");
                }
                let mut reconciled = 0usize;
                for repository in discovery.repositories {
                    let observed_at = chrono::Utc::now().to_rfc3339_opts(
                        chrono::SecondsFormat::Millis,
                        true,
                    );
                    match reconcile_one_repository(&pool, &repository, &options, &observed_at).await {
                        Ok(report) => {
                            reconciled += usize::from(report.topology.is_some());
                            healthy &= report.warnings.is_empty();
                            for warning in report.warnings {
                                tracing::warn!(path = %repository.display(), warning = ?warning,
                                    "Agent Observatory Git reconcile warning");
                            }
                        }
                        Err(error) => {
                            healthy = false;
                            tracing::error!(path = %repository.display(), error = %error,
                                "Agent Observatory Git reconcile failed");
                        }
                    }
                    if token.is_cancelled() { return; }
                }
                if healthy {
                    let completed_at = chrono::Utc::now().to_rfc3339_opts(
                        chrono::SecondsFormat::Millis,
                        true,
                    );
                    if let Err(error) = projection_cursor(&pool, "git").and_then(|_| {
                        advance_projection_cursor(&pool, "git", &completed_at)
                    }) {
                        tracing::error!(error = %error,
                            "Agent Observatory Git progress cursor advance failed");
                    }
                }
                let status = if healthy { "ok" } else { "error" };
                if let Err(error) = record_projection_health(
                    &pool,
                    "git",
                    status,
                    &format!("reconciled={reconciled}"),
                ) {
                    tracing::error!(error = %error, "Agent Observatory Git health write failed");
                }
                tracing::debug!(reconciled, "Agent Observatory Git reconcile cycle completed");
                tokio::select! {
                    biased;
                    () = token.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_secs(config.git.reconcile_interval_secs)) => {}
                }
            }
        })
    })
}

#[cfg(test)]
#[path = "agent_observatory_tests.rs"]
mod tests;
