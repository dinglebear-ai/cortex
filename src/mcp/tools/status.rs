use serde_json::{Value, json};

use super::super::AppState;

pub(in super::super) async fn tool_get_stats(
    state: &AppState,
    _args: Value,
) -> anyhow::Result<Value> {
    let stats = state.service.stats().summary().await?;
    let mut value = serde_json::to_value(&stats)?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "runtime_observability".into(),
            serde_json::to_value(state.observability.snapshot())?,
        );
        object.insert(
            "otlp".into(),
            json!({
                "logs_received": state.otlp_counters.logs_received.load(std::sync::atomic::Ordering::Relaxed),
                "decode_errors": state.otlp_counters.decode_errors.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_accepted": state.otlp_counters.metrics_accepted.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_duplicates": state.otlp_counters.metrics_duplicates.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_rejected": state.otlp_counters.metrics_rejected.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_backpressure": state.otlp_counters.metrics_backpressure.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_auth_failures": state.otlp_counters.metrics_auth_failures.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_decode_errors": state.otlp_counters.metrics_decode_errors.load(std::sync::atomic::Ordering::Relaxed),
                "metrics_persistence_errors": state.otlp_counters.metrics_persistence_errors.load(std::sync::atomic::Ordering::Relaxed),
            }),
        );
    }
    tracing::debug!(
        total_logs = stats.total_logs,
        total_hosts = stats.total_hosts,
        logical_db_size_mb = %stats.logical_db_size_mb,
        physical_db_size_mb = %stats.physical_db_size_mb,
        write_blocked = stats.write_blocked,
        phantom_fts_rows = stats.phantom_fts_rows,
        "get_stats completed"
    );
    Ok(value)
}

pub(in super::super) async fn tool_get_status(
    state: &AppState,
    _args: Value,
) -> anyhow::Result<Value> {
    let db_ok = state.service.health_check().await.is_ok();
    let db_maintenance = state.service.db_status().await.ok();
    let file_tail_statuses = state.service.file_tail_statuses_snapshot();
    let file_tail_blocked_count = file_tail_statuses
        .iter()
        .filter(|status| status.blocked_on_writer_since.is_some())
        .count();
    let degraded = db_ok && file_tail_blocked_count > 0;
    Ok(json!({
        "status": if db_ok {
            if degraded { "degraded" } else { "ok" }
        } else {
            "error"
        },
        "db_ok": db_ok,
        "db_maintenance": db_maintenance,
        "file_tails": {
            "blocked_count": file_tail_blocked_count,
            "statuses": file_tail_statuses,
        },
        "runtime_observability": state.observability.snapshot(),
        "otlp": {
            "logs_received": state.otlp_counters.logs_received.load(std::sync::atomic::Ordering::Relaxed),
            "decode_errors": state.otlp_counters.decode_errors.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_accepted": state.otlp_counters.metrics_accepted.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_duplicates": state.otlp_counters.metrics_duplicates.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_rejected": state.otlp_counters.metrics_rejected.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_backpressure": state.otlp_counters.metrics_backpressure.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_auth_failures": state.otlp_counters.metrics_auth_failures.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_decode_errors": state.otlp_counters.metrics_decode_errors.load(std::sync::atomic::Ordering::Relaxed),
            "metrics_persistence_errors": state.otlp_counters.metrics_persistence_errors.load(std::sync::atomic::Ordering::Relaxed),
        }
    }))
}
