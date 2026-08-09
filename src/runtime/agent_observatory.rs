//! Bounded, cancellable Agent Observatory projector and Git reconcile workers.

use crate::agent_observatory::projector::{
    project_agent_source, project_command_log, project_transcript_log,
};
use crate::config::AgentObservatoryConfig;
use crate::db::DbPool;
use crate::db::agent_observatory::{
    AgentSourceKind, advance_projection_cursor, page_agent_sources, projection_cursor,
    record_projection_health,
};
use crate::db::page_agent_projection_logs;
use crate::git_observer::discovery::{DiscoveryOptions, discover_repositories};
use crate::git_observer::reconcile::{ReconcileOptions, reconcile_one_repository};
use crate::scanner::local_hostname;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const SOURCE_KINDS: [AgentSourceKind; 4] = [
    AgentSourceKind::Mcp,
    AgentSourceKind::Hook,
    AgentSourceKind::Skill,
    AgentSourceKind::Llm,
];

fn source_name(kind: AgentSourceKind) -> &'static str {
    match kind {
        AgentSourceKind::Mcp => "mcp",
        AgentSourceKind::Hook => "hook",
        AgentSourceKind::Skill => "skill",
        AgentSourceKind::Llm => "llm",
    }
}

pub(super) fn spawn_projector(
    token: CancellationToken,
    pool: Arc<DbPool>,
    config: AgentObservatoryConfig,
) -> Option<JoinHandle<()>> {
    config.enabled.then(|| {
        tokio::spawn(async move {
            loop {
                if token.is_cancelled() { break; }
                let mut projected = 0usize;
                let mut healthy = true;
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
                        for row in rows {
                            page_bytes = page_bytes.saturating_add(row.message.len());
                            if page_bytes > config.projector_page_bytes { break; }
                            match (project_transcript_log(&pool, &row), project_command_log(&pool, &row)) {
                                (Ok(_), Ok(_)) => {
                                    projected += 1;
                                    if let Err(error) = advance_projection_cursor(&pool, "logs", &row.id.to_string()) {
                                        healthy = false;
                                        tracing::error!(error = %error, "Agent Observatory log cursor advance failed");
                                        break;
                                    }
                                }
                                (transcript, command) => {
                                    healthy = false;
                                    tracing::error!(transcript_error = ?transcript.err(), command_error = ?command.err(),
                                        "Agent Observatory log projection failed");
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        healthy = false;
                        tracing::error!(error = %error, "Agent Observatory log page failed");
                    }
                }
                for kind in SOURCE_KINDS {
                    let name = source_name(kind);
                    let cursor = match projection_cursor(&pool, name) {
                        Ok(cursor) => cursor,
                        Err(error) => {
                            healthy = false;
                            tracing::error!(source_kind = ?kind, error = %error,
                                "Agent Observatory source cursor load failed");
                            continue;
                        }
                    };
                    match page_agent_sources(&pool, kind, &cursor, config.projector_page_rows) {
                        Ok(page) => {
                            let mut page_bytes = 0usize;
                            for record in &page.records {
                                page_bytes = page_bytes.saturating_add(format!("{record:?}").len());
                                if page_bytes > config.projector_page_bytes {
                                    break;
                                }
                                match project_agent_source(&pool, record) {
                                    Ok(_) => projected += 1,
                                    Err(error) => {
                                        healthy = false;
                                        tracing::error!(
                                        source_kind = ?kind,
                                        error = %error,
                                        "Agent Observatory source projection failed"
                                        );
                                        break;
                                    }
                                }
                                let next_cursor = record.next_cursor();
                                if let Err(error) = advance_projection_cursor(&pool, name, &next_cursor) {
                                    healthy = false;
                                    tracing::error!(source_kind = ?kind, error = %error,
                                        "Agent Observatory source cursor advance failed");
                                    break;
                                }
                            }
                        }
                        Err(error) => {
                            healthy = false;
                            tracing::error!(source_kind = ?kind, error = %error,
                                "Agent Observatory source page failed");
                        }
                    }
                }
                let status = if healthy { "ok" } else { "error" };
                if let Err(error) = record_projection_health(
                    &pool,
                    "projector",
                    status,
                    &format!("projected={projected}"),
                ) {
                    tracing::error!(error = %error, "Agent Observatory projector health write failed");
                }
                tracing::debug!(projected, "Agent Observatory projector cycle completed");
                tokio::select! {
                    biased;
                    () = token.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_millis(config.projector_poll_ms)) => {}
                }
            }
        })
    })
}

fn expand_root(root: &str) -> PathBuf {
    root.strip_prefix("~/").map_or_else(
        || PathBuf::from(root),
        |suffix| {
            std::env::var_os("HOME")
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
