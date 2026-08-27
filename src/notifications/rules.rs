//! Hardcoded alert rules for push notifications.
//!
//! Each function takes a slice of log rows and returns `OutboxInsertParams`
//! for rows that match the rule. The evaluator calls these functions periodically.

use crate::db::notifications::{AttemptCount, OutboxInsertParams, backoff_next_attempt_at};
use crate::notifications::apprise::escape_for_notification;

/// A minimal log row used for rule evaluation.
#[derive(Debug, Clone)]
pub struct LogRow {
    pub app_name: Option<String>,
    pub message: String,
    pub hostname: String,
    pub metadata_json: Option<String>,
    pub timestamp: String,
}

/// Evaluate OOM kill events.
///
/// Matches: `app_name = 'kernel'` AND message contains `'Out of memory: Killed process'`.
pub fn evaluate_oom_kill(rows: &[LogRow], apprise_urls_json: &str) -> Vec<OutboxInsertParams> {
    rows.iter()
        .filter(|r| {
            r.app_name.as_deref() == Some("kernel")
                && r.message.contains("Out of memory: Killed process")
        })
        .map(|r| {
            let title = escape_for_notification(&format!("[CRITICAL] OOM Kill on {}", r.hostname));
            let body = escape_for_notification(&format!(
                "Kernel OOM killer fired on **{}** at {}\n\n```\n{}\n```",
                r.hostname, r.timestamp, r.message
            ));
            OutboxInsertParams {
                dedup_key: format!("oom_kill:{}", r.hostname),
                rule_id: "oom_kill".to_string(),
                severity: "critical".to_string(),
                hostname: r.hostname.clone(),
                title,
                body,
                apprise_urls_json: apprise_urls_json.to_string(),
                next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
            }
        })
        .collect()
}

/// Evaluate container die events with non-zero exit code.
///
/// Matches: `metadata_json` has `action=die` AND `exit_code != "0"`.
pub fn evaluate_container_die_nonzero(
    rows: &[LogRow],
    apprise_urls_json: &str,
) -> Vec<OutboxInsertParams> {
    rows.iter()
        .filter_map(|r| {
            let meta = r.metadata_json.as_deref()?;
            let v = serde_json::from_str::<serde_json::Value>(meta).ok()?;
            if v.get("action").and_then(|a| a.as_str()) != Some("die") {
                return None;
            }
            // Handle both string ("1") and numeric (1) exit_code values.
            let exit_code_val: Option<i64> = match v.get("exit_code") {
                Some(serde_json::Value::String(s)) => s.parse::<i64>().ok(),
                Some(serde_json::Value::Number(n)) => n.as_i64(),
                _ => None,
            };
            let is_nonzero = exit_code_val.map(|c| c != 0).unwrap_or(false);
            if !is_nonzero {
                return None;
            }
            let exit_code_str = exit_code_val
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let container = v
                .get("container_name")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown");

            let title = escape_for_notification(&format!(
                "[WARNING] Container {} died (exit {}) on {}",
                container, exit_code_str, r.hostname
            ));
            let body = escape_for_notification(&format!(
                "Container **{}** exited with code `{}` on **{}** at {}",
                container, exit_code_str, r.hostname, r.timestamp
            ));
            Some(OutboxInsertParams {
                dedup_key: format!("container_die:{}:{}", r.hostname, container),
                rule_id: "container_die_nonzero".to_string(),
                severity: "warning".to_string(),
                hostname: r.hostname.clone(),
                title,
                body,
                apprise_urls_json: apprise_urls_json.to_string(),
                next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
            })
        })
        .collect()
}

/// Evaluate fail2ban ban events.
///
/// Matches: `app_name = 'fail2ban'` AND message contains `'Ban '`.
pub fn evaluate_fail2ban_ban(rows: &[LogRow], apprise_urls_json: &str) -> Vec<OutboxInsertParams> {
    rows.iter()
        .filter(|r| r.app_name.as_deref() == Some("fail2ban") && r.message.contains("Ban "))
        .map(|r| {
            let title =
                escape_for_notification(&format!("[NOTICE] fail2ban ban on {}", r.hostname));
            let body = escape_for_notification(&format!(
                "fail2ban banned an IP on **{}** at {}\n\n{}",
                r.hostname, r.timestamp, r.message
            ));
            OutboxInsertParams {
                dedup_key: format!("fail2ban_ban:{}", r.hostname),
                rule_id: "fail2ban_ban".to_string(),
                severity: "notice".to_string(),
                hostname: r.hostname.clone(),
                title,
                body,
                apprise_urls_json: apprise_urls_json.to_string(),
                next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
            }
        })
        .collect()
}

/// Evaluate Authelia MFA failure events.
///
/// Matches: `app_name = 'authelia'` AND message contains `'second_factor'`.
pub fn evaluate_authelia_mfa_fail(
    rows: &[LogRow],
    apprise_urls_json: &str,
) -> Vec<OutboxInsertParams> {
    rows.iter()
        .filter(|r| {
            r.app_name.as_deref() == Some("authelia")
                && r.message.contains("second_factor")
                && (r.message.to_lowercase().contains("failed")
                    || r.message.to_lowercase().contains("error")
                    || r.message.to_lowercase().contains("invalid"))
        })
        .map(|r| {
            let title = escape_for_notification(&format!(
                "[WARNING] Authelia MFA failure on {}",
                r.hostname
            ));
            let body = escape_for_notification(&format!(
                "Authelia second factor event on **{}** at {}\n\n{}",
                r.hostname, r.timestamp, r.message
            ));
            OutboxInsertParams {
                dedup_key: format!("authelia_mfa:{}", r.hostname),
                rule_id: "authelia_mfa_fail".to_string(),
                severity: "warning".to_string(),
                hostname: r.hostname.clone(),
                title,
                body,
                apprise_urls_json: apprise_urls_json.to_string(),
                next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
            }
        })
        .collect()
}

/// Evaluate disk fill pressure from storage metrics.
///
/// This is NOT a log-scan rule — it takes raw bytes, not `&[LogRow]`.
/// The storage enforcement task in `src/runtime.rs` calls this function
/// directly after each `enforce_storage_budget` cycle.
///
/// Fires when `free_bytes` is below the configured guardrail thresholds:
///   - `free_bytes < critical_bytes` → "critical"
///   - `free_bytes < warn_bytes`     → "warning"
///   - otherwise                     → `None`
///
/// `critical_bytes` = `min_free_disk_mb * 1024 * 1024` from StorageConfig.
/// `warn_bytes`     = `recovery_free_disk_mb * 1024 * 1024` from StorageConfig.
///
/// Pass `critical_bytes = 0` or `warn_bytes = 0` to disable that threshold.
pub fn evaluate_disk_fill(
    hostname: &str,
    free_bytes: u64,
    critical_bytes: u64,
    warn_bytes: u64,
    apprise_urls_json: &str,
) -> Option<OutboxInsertParams> {
    let (severity, label) = if critical_bytes > 0 && free_bytes < critical_bytes {
        ("critical", "CRITICAL")
    } else if warn_bytes > 0 && free_bytes < warn_bytes {
        ("warning", "WARNING")
    } else {
        return None;
    };
    let free_mib = free_bytes / (1024 * 1024);
    let title = escape_for_notification(&format!(
        "[{label}] Disk fill on {hostname}: {free_mib} MiB free"
    ));
    let body = escape_for_notification(&format!(
        "Host **{hostname}** has only {free_mib} MiB disk space remaining."
    ));
    Some(OutboxInsertParams {
        dedup_key: format!("disk_fill:{hostname}:{severity}"),
        rule_id: "disk_fill".to_string(),
        severity: severity.to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
    })
}

/// Evaluate ingest queue pressure from runtime counters.
///
/// Fires when queue-full transitions or UDP queue-full drops have increased
/// since the previous evaluation cycle. TCP uses awaited backpressure and does
/// not drop lines when the ingest queue is full.
pub fn evaluate_ingest_queue_pressure(
    hostname: &str,
    full_transitions_delta: u64,
    udp_drops_delta: u64,
    queue_depth: usize,
    queue_capacity: usize,
    apprise_urls_json: &str,
) -> Option<OutboxInsertParams> {
    if full_transitions_delta == 0 && udp_drops_delta == 0 {
        return None;
    }

    let title = escape_for_notification(&format!(
        "[WARNING] syslog ingest queue pressure on {hostname}"
    ));
    let body = escape_for_notification(&format!(
        "cortex observed queue pressure on **{hostname}** since the last check:\n\n\
         - queue-full transitions: `{full_transitions_delta}`\n\
         - UDP drops from full queue: `{udp_drops_delta}`\n\
         - current queue depth: `{queue_depth}/{queue_capacity}`"
    ));

    Some(OutboxInsertParams {
        dedup_key: format!("ingest_queue_pressure:{hostname}"),
        rule_id: "ingest_queue_pressure".to_string(),
        severity: "warning".to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
    })
}

/// Evaluate ingest silence — the push-path complement to a dead listener.
///
/// This is NOT a log-scan rule — like `evaluate_disk_fill` it takes a metric:
/// the age in seconds of the newest ingested row. The evaluator computes it
/// from `MAX(received_at)` each cycle.
///
/// Fires when the DB has logs but the newest row is older than
/// `threshold_secs`. A database with no rows at all does NOT fire — that is
/// "ingest never started" (fresh install, no forwarders yet), not "ingest
/// stopped", and alerting on it would page every fresh deployment.
///
/// `ingest_queue_pressure` covers the opposite failure mode (queue full);
/// this rule covers the queue staying empty because listeners are dead or the
/// forwarding chain broke (bead syslog-mcp-7f0y).
pub fn evaluate_ingest_silence(
    hostname: &str,
    newest_row_age_secs: Option<u64>,
    threshold_secs: u64,
    apprise_urls_json: &str,
) -> Option<OutboxInsertParams> {
    let age_secs = newest_row_age_secs?;
    if threshold_secs == 0 || age_secs < threshold_secs {
        return None;
    }
    let age_mins = age_secs / 60;
    let title = escape_for_notification(&format!(
        "[CRITICAL] cortex ingest silent on {hostname}: no logs for {age_mins} min"
    ));
    let body = escape_for_notification(&format!(
        "cortex on **{hostname}** has not ingested any log rows for {age_mins} minutes \
         (threshold: {} min).\n\n\
         Likely causes: dead syslog listener, broken rsyslog forwarding, or a \
         write-blocked database. Check `/health`, `cortex action=ingest_rate`, \
         and `cortex action=silent_hosts`.",
        threshold_secs / 60
    ));
    Some(OutboxInsertParams {
        dedup_key: format!("ingest_silence:{hostname}"),
        rule_id: "ingest_silence".to_string(),
        severity: "critical".to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
    })
}

/// Evaluate heartbeat silence for one host — the agent-level complement to
/// `evaluate_ingest_silence` (bead syslog-mcp-5uqus).
///
/// Like the other metric rules this takes precomputed inputs: the evaluator
/// reads stale hosts from `host_heartbeats_latest` (already bounded by
/// threshold and forget horizon in SQL) and calls this per host.
///
/// **Fires once per outage**: `last_heartbeat_at` rides in the dedup key, so
/// the same outage maps to the same key forever — the dispatcher's firings
/// dedup then drops re-enqueues. A recovery followed by a new outage changes
/// `last_heartbeat_at` and produces a fresh key.
pub fn evaluate_heartbeat_silence(
    host_id: &str,
    hostname: &str,
    last_heartbeat_at: &str,
    age_secs: u64,
    threshold_secs: u64,
    apprise_urls_json: &str,
) -> OutboxInsertParams {
    let age_mins = age_secs / 60;
    let title = escape_for_notification(&format!(
        "[CRITICAL] cortex agent silent on {hostname}: no heartbeat for {age_mins} min"
    ));
    let body = escape_for_notification(&format!(
        "The cortex agent on **{hostname}** has not delivered a heartbeat for \
         {age_mins} minutes (threshold: {} min). Last accepted heartbeat: \
         {last_heartbeat_at}.\n\n\
         Likely causes: agent process/container down, host offline or \
         rebooting, or the heartbeat target unreachable from the host. Check \
         `cortex action=fleet_state` and the agent service on the host.",
        threshold_secs / 60
    ));
    OutboxInsertParams {
        // Keyed on host_id, not hostname — host_id is the stable identity
        // (hostnames can collide across agents); the display fields stay
        // human-readable.
        dedup_key: format!("heartbeat_silence:{host_id}:{last_heartbeat_at}"),
        rule_id: "heartbeat_silence".to_string(),
        severity: "critical".to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
    }
}

/// Evaluate stream silence for one previously-active log stream.
///
/// The evaluator maintains `stream_last_seen` (newest row per hostname +
/// source kind) and calls this for entries older than the threshold but
/// inside the forget horizon, restricted to the configured kind allowlist.
/// Same once-per-outage dedup shape as [`evaluate_heartbeat_silence`]: the
/// stalled `last_seen_at` value keys the outage.
pub fn evaluate_stream_silence(
    hostname: &str,
    source_kind: &str,
    last_seen_at: &str,
    age_secs: u64,
    threshold_secs: u64,
    apprise_urls_json: &str,
) -> OutboxInsertParams {
    let age_mins = age_secs / 60;
    let title = escape_for_notification(&format!(
        "[WARNING] cortex stream silent: {source_kind} from {hostname} stopped {age_mins} min ago"
    ));
    let body = escape_for_notification(&format!(
        "**{hostname}** had been sending `{source_kind}` logs but none have \
         arrived for {age_mins} minutes (threshold: {} min). Last row: \
         {last_seen_at}.\n\n\
         The host is otherwise reachable if its heartbeat is current — check \
         the specific forwarder (docker socket access, tailed file rotation, \
         syslog forwarding) on the host. `cortex action=filter` with \
         host/source_kind narrows it down.",
        threshold_secs / 60
    ));
    OutboxInsertParams {
        dedup_key: format!("stream_silence:{hostname}:{source_kind}:{last_seen_at}"),
        rule_id: "stream_silence".to_string(),
        severity: "warning".to_string(),
        hostname: hostname.to_string(),
        title,
        body,
        apprise_urls_json: apprise_urls_json.to_string(),
        next_attempt_at: backoff_next_attempt_at(AttemptCount::NONE),
    }
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
