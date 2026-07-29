use rusqlite::{Error as SqliteError, ErrorCode};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info};

use super::enrichment::{EnrichmentConfig, enrich_entry};
use crate::config::StorageConfig;
use crate::db::{self, DbPool};
use crate::enrich::EnrichmentPipeline;
use crate::ingest::IngestEnvelope;
use crate::observability::RuntimeObservability;

const INGEST_SUMMARY_INTERVAL_SECS: u64 = 60;
const FAILED_BATCH_RETAIN_LIMIT: usize = 1000;
const FAILED_BATCH_RETRY_CHUNK_SIZE: usize = 100;
const INGEST_SUMMARY_CARDINALITY_LIMIT: usize = 256;
const OTHER_SUMMARY_LABEL: &str = "__other__";

/// Batch writer — collects messages and writes in batches for throughput.
pub(crate) struct WriterContext {
    pool: Arc<DbPool>,
    storage: StorageConfig,
    storage_state: Arc<Mutex<Option<db::StorageBudgetState>>>,
    enrichment: EnrichmentConfig,
    pub pipeline: Arc<EnrichmentPipeline>,
    observability: Arc<RuntimeObservability>,
}

impl WriterContext {
    pub(crate) fn new(
        pool: Arc<DbPool>,
        storage: StorageConfig,
        storage_state: Arc<Mutex<Option<db::StorageBudgetState>>>,
        enrichment: EnrichmentConfig,
        pipeline: Arc<EnrichmentPipeline>,
        observability: Arc<RuntimeObservability>,
    ) -> Self {
        Self {
            pool,
            storage,
            storage_state,
            enrichment,
            pipeline,
            observability,
        }
    }
}

pub(crate) async fn batch_writer(
    mut rx: mpsc::Receiver<IngestEnvelope>,
    context: WriterContext,
    batch_size: usize,
    flush_interval: tokio::time::Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut batch: Vec<IngestEnvelope> = Vec::with_capacity(batch_size);
    let mut storage_blocked = false;
    let mut summary = IngestSummary::default();
    let mut summary_deadline = tokio::time::Instant::now()
        + tokio::time::Duration::from_secs(INGEST_SUMMARY_INTERVAL_SECS);
    info!(
        batch_size,
        flush_interval_ms = flush_interval.as_millis(),
        "Batch writer started"
    );

    loop {
        let deadline = tokio::time::sleep(flush_interval);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(parsed) => {
                            let requires_durable_ack = parsed.requires_durable_ack();
                            batch.push(parsed);
                            debug!(
                                batch_len = batch.len(),
                                queue_depth = rx.max_capacity().saturating_sub(rx.capacity()),
                                queue_capacity = rx.max_capacity(),
                                "Queued parsed syslog entry"
                            );
                            if requires_durable_ack || (!batch.is_empty() && batch.len().is_multiple_of(batch_size)) {
                                break;
                            }
                        }
                        None => {
                            if !batch.is_empty() {
                                flush_batch(
                                    &mut batch,
                                    &mut storage_blocked,
                                    &mut summary,
                                    &context,
                                )
                                .await;
                            }
                            emit_ingest_summary(&mut summary);
                            info!("Write channel closed, exiting batch writer");
                            return;
                        }
                    }
                }
                _ = &mut deadline => {
                    break;
                }
                // Cooperative shutdown: drain remaining entries from the channel
                // then flush one final batch before exiting. This prevents log
                // loss when the runtime receives SIGTERM.
                Ok(()) = shutdown.changed(), if *shutdown.borrow() => {
                    // Drain whatever is already in the channel without blocking.
                    while let Ok(entry) = rx.try_recv() {
                        batch.push(entry);
                    }
                    if !batch.is_empty() {
                        info!(batch_len = batch.len(), "Flushing residual batch on shutdown");
                        flush_batch(&mut batch, &mut storage_blocked, &mut summary, &context).await;
                    }
                    emit_ingest_summary(&mut summary);
                    info!("Batch writer shutdown cleanly");
                    return;
                }
            }
        }

        if !batch.is_empty() {
            flush_batch(&mut batch, &mut storage_blocked, &mut summary, &context).await;
        }

        if tokio::time::Instant::now() >= summary_deadline {
            emit_ingest_summary(&mut summary);
            summary_deadline = tokio::time::Instant::now()
                + tokio::time::Duration::from_secs(INGEST_SUMMARY_INTERVAL_SECS);
        }
    }
}

pub(super) async fn flush_batch(
    batch: &mut Vec<IngestEnvelope>,
    storage_blocked: &mut bool,
    summary: &mut IngestSummary,
    context: &WriterContext,
) {
    let pool = Arc::clone(&context.pool);
    // Enrichment runs in the async context (cheap regex/JSON work) so the
    // spawn_blocking call below stays focused on the SQL write.
    // Two enrichment frameworks run in sequence on every entry.
    // They write to NON-OVERLAPPING fields, so running both is safe:
    //
    //   enrich_entry() (src/syslog/enrichment.rs — legacy framework):
    //     severity, app_name, ai_tool, ai_project, ai_session_id,
    //     ai_transcript_path
    //
    //   pipeline.dispatch() (src/enrich/ — new framework, Arch-H5):
    //     http_status, auth_outcome, dns_blocked, event_action, parse_error
    //
    // Migration path: once equivalence tests confirm the new framework
    // covers all fields written by enrich_entry(), remove the legacy call.
    // Until then, both calls are required. The double-parse inside each
    // framework (metadata_json parsed twice per entry) was fixed in the
    // new framework's dispatch() — it now parses once and passes a
    // reference to all helpers (Arch-H5 partial fix, 2026-05-22).
    let batch_to_write: Vec<IngestEnvelope> = std::mem::take(batch)
        .into_iter()
        .map(|mut envelope| {
            envelope.entry = enrich_entry(envelope.entry, &context.enrichment);
            context.pipeline.dispatch(&mut envelope.entry);
            envelope
        })
        .collect();
    let count = batch_to_write.len();
    let started = Instant::now();
    debug!(count, "Attempting batch flush");
    let enforcement = context.storage_state.lock().clone();
    if let Some(state) = enforcement
        && state.write_blocked
    {
        let err = anyhow::anyhow!(
            "storage budget exceeded: logical_db_size_bytes={}, free_disk_bytes={:?}",
            state.metrics.logical_db_size_bytes,
            state.metrics.free_disk_bytes
        );
        if !*storage_blocked {
            error!(
                error = %err,
                count,
                retained_batch = batch_to_write.len(),
                elapsed_ms = started.elapsed().as_millis(),
                max_db_size_mb = context.storage.max_db_size_mb,
                min_free_disk_mb = context.storage.min_free_disk_mb,
                "Storage budget exceeded — retaining batch until space recovers"
            );
            *storage_blocked = true;
        }
        context
            .observability
            .record_writer_retained(batch_to_write.len(), true);
        *batch = batch_to_write;
        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        return;
    }
    match tokio::task::spawn_blocking(move || {
        match insert_envelopes_batch(&pool, &batch_to_write) {
            Ok(n) => Ok((n, batch_to_write)),
            Err(e) => {
                let outcome = handle_failed_batch(&pool, batch_to_write, &e);
                Err((e, outcome))
            }
        }
    })
    .await
    {
        Ok(Ok((n, inserted_batch))) => {
            let inserted_count = n.min(inserted_batch.len());
            summary.record_envelopes(&inserted_batch[..inserted_count]);
            context.observability.record_writer_flushed(n);
            if *storage_blocked {
                info!(
                    count = n,
                    elapsed_ms = started.elapsed().as_millis(),
                    "storage budget recovered — writes resumed"
                );
                *storage_blocked = false;
            }
            debug!(
                count = n,
                elapsed_ms = started.elapsed().as_millis(),
                "Flushed log batch"
            );
            for envelope in inserted_batch.into_iter().take(inserted_count) {
                envelope.ack_success();
            }
        }
        Ok(Err((e, outcome))) => {
            if !outcome.inserted_entries.is_empty() {
                let inserted_count = outcome.inserted_count.min(outcome.inserted_entries.len());
                summary.record_envelopes(&outcome.inserted_entries[..inserted_count]);
                context.observability.record_writer_flushed(inserted_count);
                debug!(
                    inserted_count,
                    elapsed_ms = started.elapsed().as_millis(),
                    "Flushed recovered log chunks after batch failure"
                );
            }

            for envelope in outcome.inserted_entries {
                envelope.ack_success();
            }

            if !outcome.retained_entries.is_empty() {
                context
                    .observability
                    .record_writer_retained(outcome.retained_entries.len(), false);
                if !*storage_blocked && is_disk_full_error(&e) {
                    error!(
                        error = %e,
                        count,
                        retained_rows = outcome.retained_entries.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "SQLite disk full — retaining batch until space recovers"
                    );
                }
                error!(
                    error = %e,
                    count,
                    inserted_rows = outcome.inserted_count,
                    retained_rows = outcome.retained_entries.len(),
                    retained_chunks = outcome.retained_chunks,
                    discarded_rows = outcome.discarded_count,
                    discarded_chunks = outcome.discarded_chunks,
                    elapsed_ms = started.elapsed().as_millis(),
                    "Failed to flush full log batch — retained retryable chunks and discarded unrecoverable rows"
                );
                *batch = outcome.retained_entries;
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            }

            if outcome.discarded_count > 0 {
                context
                    .observability
                    .record_writer_discarded(outcome.discarded_count);
                error!(
                    error = %e,
                    count,
                    inserted_rows = outcome.inserted_count,
                    retained_rows = batch.len(),
                    discarded_rows = outcome.discarded_count,
                    discarded_chunks = outcome.discarded_chunks,
                    elapsed_ms = started.elapsed().as_millis(),
                    "Discarded unrecoverable log rows after failed batch retry"
                );
            }
        }
        Err(e) => {
            context.observability.record_writer_discarded(count);
            error!(
                error = %e,
                count,
                elapsed_ms = started.elapsed().as_millis(),
                "spawn_blocking panicked during flush — batch discarded"
            );
        }
    }
}

fn insert_envelopes_batch(pool: &DbPool, envelopes: &[IngestEnvelope]) -> anyhow::Result<usize> {
    let entries = envelopes
        .iter()
        .map(|envelope| envelope.entry.clone())
        .collect::<Vec<_>>();
    db::insert_logs_batch(pool, &entries)
}

#[derive(Default)]
struct FailedBatchOutcome {
    inserted_count: usize,
    inserted_entries: Vec<IngestEnvelope>,
    retained_entries: Vec<IngestEnvelope>,
    retained_chunks: usize,
    discarded_count: usize,
    discarded_chunks: usize,
}

fn handle_failed_batch(
    pool: &DbPool,
    failed_batch: Vec<IngestEnvelope>,
    error: &anyhow::Error,
) -> FailedBatchOutcome {
    let mut outcome = FailedBatchOutcome::default();
    if is_retryable_sqlite_error(error) {
        retain_or_discard_entries(&mut outcome, failed_batch);
        return outcome;
    }

    let mut remaining = failed_batch;
    while !remaining.is_empty() {
        let next = if remaining.len() > FAILED_BATCH_RETRY_CHUNK_SIZE {
            remaining.split_off(FAILED_BATCH_RETRY_CHUNK_SIZE)
        } else {
            Vec::new()
        };
        retry_failed_chunk(pool, remaining, &mut outcome);
        remaining = next;
    }
    outcome
}

fn retry_failed_chunk(pool: &DbPool, chunk: Vec<IngestEnvelope>, outcome: &mut FailedBatchOutcome) {
    match insert_envelopes_batch(pool, &chunk) {
        Ok(inserted) => {
            outcome.inserted_count += inserted;
            outcome.inserted_entries.extend(chunk);
        }
        Err(e) if is_retryable_sqlite_error(&e) => {
            retain_or_discard_entries(outcome, chunk);
        }
        Err(_) if chunk.len() > 1 => {
            let mid = chunk.len() / 2;
            let mut right = chunk;
            let left = right.split_off(mid);
            retry_failed_chunk(pool, right, outcome);
            retry_failed_chunk(pool, left, outcome);
        }
        Err(_) => {
            outcome.discarded_count += chunk.len();
            outcome.discarded_chunks += 1;
            for envelope in chunk {
                envelope.ack_failure("unrecoverable database insert failure");
            }
        }
    }
}

fn retain_or_discard_entries(outcome: &mut FailedBatchOutcome, entries: Vec<IngestEnvelope>) {
    let retain_remaining = FAILED_BATCH_RETAIN_LIMIT.saturating_sub(outcome.retained_entries.len());
    if retain_remaining == 0 {
        let discarded = entries.len();
        for envelope in entries {
            tracing::warn!(
                hostname = %envelope.entry.hostname,
                severity = %envelope.entry.severity,
                timestamp = %envelope.entry.timestamp,
                "Discarding log entry: retain limit reached"
            );
            envelope.ack_failure("ingest writer retain limit reached");
        }
        outcome.discarded_count += discarded;
        outcome.discarded_chunks += 1;
        return;
    }

    if entries.len() <= retain_remaining {
        outcome.retained_entries.extend(entries);
        outcome.retained_chunks += 1;
        return;
    }

    let discarded = entries.len() - retain_remaining;
    let mut iter = entries.into_iter();
    outcome
        .retained_entries
        .extend(iter.by_ref().take(retain_remaining));
    outcome.retained_chunks += 1;
    for envelope in iter {
        tracing::warn!(
            hostname = %envelope.entry.hostname,
            severity = %envelope.entry.severity,
            timestamp = %envelope.entry.timestamp,
            "Discarding log entry: retain limit reached"
        );
        envelope.ack_failure("ingest writer retain limit reached");
    }
    outcome.discarded_count += discarded;
    outcome.discarded_chunks += 1;
}

fn is_retryable_sqlite_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<SqliteError>(),
            Some(SqliteError::SqliteFailure(sql_err, _))
                if matches!(
                    sql_err.code,
                    ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked | ErrorCode::DiskFull
                )
        )
    })
}

fn is_disk_full_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<SqliteError>(),
            Some(SqliteError::SqliteFailure(sql_err, _))
                if sql_err.code == ErrorCode::DiskFull
        )
    })
}

#[derive(Default)]
pub(super) struct IngestSummary {
    total_logs: usize,
    host_counts: HashMap<String, usize>,
    source_ip_counts: HashMap<String, usize>,
    sender_counts: HashMap<(String, String), usize>,
    host_overflow_count: usize,
    source_ip_overflow_count: usize,
    sender_overflow_count: usize,
}

impl IngestSummary {
    fn record_envelopes(&mut self, envelopes: &[IngestEnvelope]) {
        self.total_logs += envelopes.len();
        for envelope in envelopes {
            self.record_entry(&envelope.entry);
        }
    }

    #[cfg(test)]
    fn record_batch(&mut self, entries: &[db::LogBatchEntry]) {
        self.total_logs += entries.len();
        for entry in entries {
            self.record_entry(entry);
        }
    }

    fn record_entry(&mut self, entry: &db::LogBatchEntry) {
        self.host_overflow_count +=
            record_bounded_count(&mut self.host_counts, entry.hostname.clone());
        let source_ip = source_addr_ip(&entry.source_ip);
        self.source_ip_overflow_count +=
            record_bounded_count(&mut self.source_ip_counts, source_ip.clone());
        self.sender_overflow_count +=
            record_bounded_count(&mut self.sender_counts, (entry.hostname.clone(), source_ip));
    }

    fn reset(&mut self) {
        self.total_logs = 0;
        self.host_counts.clear();
        self.source_ip_counts.clear();
        self.sender_counts.clear();
        self.host_overflow_count = 0;
        self.source_ip_overflow_count = 0;
        self.sender_overflow_count = 0;
    }
}

fn record_bounded_count<K>(counts: &mut HashMap<K, usize>, key: K) -> usize
where
    K: Eq + std::hash::Hash,
{
    if let Some(count) = counts.get_mut(&key) {
        *count += 1;
        return 0;
    }
    if counts.len() < INGEST_SUMMARY_CARDINALITY_LIMIT {
        counts.insert(key, 1);
        return 0;
    }
    1
}

fn emit_ingest_summary(summary: &mut IngestSummary) {
    if summary.total_logs == 0 {
        return;
    }

    let top_senders =
        summarize_top_senders(&summary.sender_counts, summary.sender_overflow_count, 5);
    info!(
        interval_secs = INGEST_SUMMARY_INTERVAL_SECS,
        total_logs = summary.total_logs,
        unique_hosts = summary.host_counts.len(),
        unique_source_ips = summary.source_ip_counts.len(),
        host_overflow_count = summary.host_overflow_count,
        source_ip_overflow_count = summary.source_ip_overflow_count,
        sender_overflow_count = summary.sender_overflow_count,
        top_senders = %top_senders,
        "Syslog ingest summary"
    );
    summary.reset();
}

pub(super) fn summarize_top_senders(
    counts: &HashMap<(String, String), usize>,
    overflow_count: usize,
    limit: usize,
) -> String {
    let overflow = (
        (
            OTHER_SUMMARY_LABEL.to_string(),
            OTHER_SUMMARY_LABEL.to_string(),
        ),
        overflow_count,
    );
    let mut entries: Vec<_> = counts.iter().collect();
    if overflow_count > 0 {
        entries.push((&overflow.0, &overflow.1));
    }
    entries.sort_by(|a, b| {
        b.1.cmp(a.1)
            .then_with(|| a.0.0.cmp(&b.0.0))
            .then_with(|| a.0.1.cmp(&b.0.1))
    });
    entries
        .into_iter()
        .take(limit)
        .map(|((host, source_ip), count)| format!("{host}@{source_ip}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn source_addr_ip(source_addr: &str) -> String {
    source_addr
        .parse::<std::net::SocketAddr>()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| source_addr.to_string())
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
