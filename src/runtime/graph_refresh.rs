//! Background scheduler for the derived investigation graph projection.
//!
//! The graph is a rebuildable projection over `logs`, heartbeats, and error
//! signatures. Historically it was only ever (re)built by the local-only
//! `cortex graph rebuild` CLI command, so a long-lived server's graph drifted
//! stale until an operator ran a full rebuild by hand — and running that CLI as
//! a second process against the live DB risked dual-writer contention.
//!
//! This opt-in task keeps the graph current from inside the server process,
//! where it shares the process-global `write_lock` with the ingest writer:
//!
//! - On startup it does an eager pass: a full build if the projection has never
//!   been built (or is degraded), otherwise an incremental delta.
//! - Thereafter it runs an incremental pass every
//!   `CORTEX_GRAPH_REFRESH_INTERVAL_SECS` (`0` disables the task and is the
//!   default).
//!
//! Incremental passes process only logs newer than the recorded watermark plus
//! the bounded heartbeat/signature snapshots, so steady-state cost is small. The
//! one-time full build is the only long pass; it builds into per-connection TEMP
//! staging without holding the write lock, so ingestion keeps flowing and only
//! the final merge transaction briefly blocks writes.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::{self, DbPool};
use crate::observability::RuntimeObservability;

use super::background_interval;

/// Default cadence for incremental graph projection refreshes. Keep this
/// disabled unless explicitly enabled: even "incremental" passes can spend
/// minutes merging snapshot-derived graph rows on large production databases.
/// The `cortex graph rebuild` CLI remains available for manual reconciles.
const GRAPH_REFRESH_INTERVAL_SECS: u64 = 0;

/// Delay before the eager startup pass, so it does not contend with the burst of
/// schema/migration and other startup work.
const GRAPH_REFRESH_STARTUP_DELAY_SECS: u64 = 20;

pub fn spawn(
    token: CancellationToken,
    pool: Arc<DbPool>,
    maintenance_limiter: Arc<Semaphore>,
    observability: Arc<RuntimeObservability>,
) -> Option<JoinHandle<()>> {
    let interval_secs = refresh_interval_secs();
    if interval_secs == 0 {
        tracing::info!("graph_refresh: disabled");
        return None;
    }
    Some(tokio::spawn(async move {
        let mut interval = background_interval(tokio::time::Duration::from_secs(interval_secs));
        let mut eager = true;
        loop {
            if eager {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::debug!("graph_refresh: cancelled before first pass");
                        break;
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(
                        GRAPH_REFRESH_STARTUP_DELAY_SECS,
                    )) => {}
                }
                eager = false;
            } else {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::debug!("graph_refresh: cooperative shutdown");
                        break;
                    }
                    _ = interval.tick() => {}
                }
            }
            observability.record_task_tick("graph_refresh");
            run_refresh(&pool, &maintenance_limiter).await;
        }
    }))
}

async fn run_refresh(pool: &Arc<DbPool>, maintenance_limiter: &Arc<Semaphore>) {
    let Ok(_permit) = Arc::clone(maintenance_limiter).acquire_owned().await else {
        tracing::error!("graph_refresh: maintenance limiter closed");
        return;
    };
    let pool = Arc::clone(pool);
    let started = Instant::now();
    let outcome =
        tokio::task::spawn_blocking(move || db::graph::refresh_graph_projection_incremental(&pool))
            .await;
    match outcome {
        Ok(Ok(db::graph::GraphRebuildOutcome::Rebuilt(stats))) => tracing::info!(
            entities = stats.entity_count,
            relationships = stats.relationship_count,
            evidence = stats.evidence_count,
            source_rows = stats.source_row_count,
            chunk_count = stats.chunk_count,
            elapsed_ms = started.elapsed().as_millis(),
            "graph_refresh: projection refreshed"
        ),
        Ok(Ok(db::graph::GraphRebuildOutcome::AlreadyRunning)) => {
            tracing::debug!("graph_refresh: another projection run is in progress; skipping")
        }
        Ok(Err(error)) => tracing::warn!(
            %error,
            elapsed_ms = started.elapsed().as_millis(),
            "graph_refresh: projection refresh failed"
        ),
        Err(join_error) => tracing::error!(
            %join_error,
            "graph_refresh: projection task panicked"
        ),
    }
}

fn refresh_interval_secs() -> u64 {
    crate::env::var("CORTEX_GRAPH_REFRESH_INTERVAL_SECS")
        .ok()
        .as_deref()
        .map(str::trim)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(GRAPH_REFRESH_INTERVAL_SECS)
}

#[cfg(test)]
#[path = "graph_refresh_tests.rs"]
mod tests;
