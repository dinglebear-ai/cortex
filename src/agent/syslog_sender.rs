//! Durable, receipt-backed syslog forwarding for Cortex agents.
//!
//! TCP syslog is deliberately not used for the reliable path: it has no
//! receipt semantics. Frames are persisted locally, POSTed to the authenticated
//! receiver, and removed only after their exact idempotency receipt arrives.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use chrono::{SecondsFormat, Utc};
use getrandom::fill as random_fill;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio::time::sleep;

use crate::syslog_forward_ingest::{
    SyslogForwardGap, SyslogForwardRecord, SyslogForwardRequest, SyslogForwardResponse,
};

pub(super) const MAX_SOURCE_SPOOL_RECORDS: usize = 1_024;
pub(super) const MAX_SOURCE_SPOOL_BYTES: usize = 1024 * 1024;
pub(super) const MAX_SPOOL_RECORDS: usize = 4_096;
pub(super) const MAX_SPOOL_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_SPOOL_AGE_SECS: i64 = 86_400;
const MAX_BATCH_RECORDS: usize = 100;
const MAX_BATCH_BYTES: usize = 512 * 1024;
const MAX_FORWARD_RECORD_BYTES: usize = 64 * 1024;
const RECONNECT_MAX_MS: u64 = 30_000;

/// Payload-free forwarding health that is safe to log or expose in status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyslogForwardStatus {
    pub queued_records: usize,
    pub queued_bytes: usize,
    pub pending_gaps: usize,
    pub oldest_age_secs: Option<u64>,
    pub evicted_records: u64,
    pub last_error_code: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SpoolState {
    source_instance: String,
    #[serde(default = "default_epoch")]
    source_epoch: u64,
    #[serde(default)]
    next_sequence: u64,
    #[serde(default)]
    next_sequences: HashMap<String, u64>,
    #[serde(default)]
    records: VecDeque<SyslogForwardRecord>,
    #[serde(default)]
    gaps: VecDeque<SyslogForwardGap>,
    #[serde(default)]
    evicted_records: u64,
    #[serde(default)]
    last_dispatched_source: Option<String>,
}

#[path = "syslog_sender/spool.rs"]
mod spool;
use spool::{
    evict_aggregate_to_quota, evict_source_to_quota, load_spool, next_source_key, save_spool,
    source_key_of,
};

fn default_epoch() -> u64 {
    1
}

impl Default for SpoolState {
    fn default() -> Self {
        Self {
            source_instance: random_source_instance(),
            source_epoch: 1,
            next_sequence: 0,
            next_sequences: HashMap::new(),
            records: VecDeque::new(),
            gaps: VecDeque::new(),
            evicted_records: 0,
            last_dispatched_source: None,
        }
    }
}

struct SenderState {
    spool_path: PathBuf,
    spool: SpoolState,
    /// An unreadable original spool is an operator-recovery condition, not a
    /// transient delivery error. Retain the status signal even after later
    /// batches succeed through the separate recovery spool.
    recovery_required: bool,
    last_error_code: Option<&'static str>,
}

/// Local collection never waits for the remote receiver. A full/outage path
/// stays bounded and creates a durable payload-free gap before any eviction.
pub struct SyslogSender {
    state: Arc<Mutex<SenderState>>,
    notify: Arc<Notify>,
    /// The sender is normally retained by the agent for its entire lifetime.
    /// Keeping the abort handle means a dropped/restarted agent cannot leave an
    /// orphaned delivery loop running against the durable spool.
    delivery_task: tokio::task::AbortHandle,
}

impl SyslogSender {
    pub fn new(target: String, token: Option<String>, spool_path: PathBuf) -> Self {
        let loaded = load_spool(&spool_path);
        let state = Arc::new(Mutex::new(SenderState {
            spool: loaded.spool,
            spool_path: loaded.spool_path,
            recovery_required: loaded.error_code.is_some(),
            last_error_code: loaded.error_code,
        }));
        let notify = Arc::new(Notify::new());
        let delivery_task = tokio::spawn(delivery_loop(
            target,
            token,
            Arc::clone(&state),
            Arc::clone(&notify),
        ));
        Self {
            state,
            notify,
            delivery_task: delivery_task.abort_handle(),
        }
    }

    pub async fn send(&self, line: String) -> Result<()> {
        self.send_from("syslog", line).await
    }

    pub async fn send_from(&self, source_key: &str, line: String) -> Result<()> {
        self.enqueue(source_key, line)
    }

    /// Compatibility API for high-rate tailers. Failure is not silent: it is a
    /// payload-free diagnostic and the caller's stream can restart normally.
    pub fn try_send(&self, line: String) {
        self.try_send_from("syslog", line);
    }

    pub fn try_send_from(&self, source_key: &str, line: String) {
        if let Err(error) = self.enqueue(source_key, line) {
            tracing::error!(
                reason_code = "local_spool_persist_failed",
                source_key = %stable_source_key(source_key),
                error = format!("{error:#}"),
                "syslog frame could not be retained"
            );
        }
    }

    pub fn status(&self) -> SyslogForwardStatus {
        let state = self.state.lock().expect("syslog sender state poisoned");
        let oldest_age_secs = state.spool.records.front().and_then(|record| {
            chrono::DateTime::parse_from_rfc3339(&record.observed_at)
                .ok()
                .map(|at| {
                    Utc::now()
                        .signed_duration_since(at.with_timezone(&Utc))
                        .num_seconds()
                        .max(0) as u64
                })
        });
        SyslogForwardStatus {
            queued_records: state.spool.records.len(),
            queued_bytes: state.spool.records.iter().map(|r| r.line.len()).sum(),
            pending_gaps: state.spool.gaps.len(),
            oldest_age_secs,
            evicted_records: state.spool.evicted_records,
            last_error_code: state.last_error_code,
        }
    }

    fn enqueue(&self, source_key: &str, line: String) -> Result<()> {
        let mut state = self.state.lock().expect("syslog sender state poisoned");
        let source_key = stable_source_key(source_key);
        let sequence = {
            let legacy_next = state.spool.next_sequence;
            let next = state
                .spool
                .next_sequences
                .entry(source_key.clone())
                .or_insert(legacy_next);
            *next = next.saturating_add(1);
            *next
        };
        let source_instance = format!("{}:{source_key}", state.spool.source_instance);
        let epoch = state.spool.source_epoch;
        if line.len() > MAX_FORWARD_RECORD_BYTES {
            state.spool.gaps.push_back(SyslogForwardGap {
                source_instance: source_instance.clone(),
                source_epoch: epoch,
                from_sequence: sequence,
                to_sequence: sequence,
                idempotency_key: delivery_key(&source_instance, epoch, sequence, "gap"),
                observed_at: now(),
                reason_code: "record_too_large".into(),
            });
            state.spool.evicted_records = state.spool.evicted_records.saturating_add(1);
            while state.spool.gaps.len() > 256 {
                state.spool.gaps.pop_front();
            }
        } else {
            state.spool.records.push_back(SyslogForwardRecord {
                source_instance: source_instance.clone(),
                source_epoch: epoch,
                sequence,
                idempotency_key: delivery_key(&source_instance, epoch, sequence, "record"),
                observed_at: now(),
                line,
            });
        }
        evict_source_to_quota(&mut state.spool, &source_key);
        evict_aggregate_to_quota(&mut state.spool);
        save_spool(&state.spool_path, &state.spool)?;
        drop(state);
        self.notify.notify_one();
        Ok(())
    }
}

impl Drop for SyslogSender {
    fn drop(&mut self) {
        self.delivery_task.abort();
    }
}

async fn delivery_loop(
    target: String,
    token: Option<String>,
    state: Arc<Mutex<SenderState>>,
    notify: Arc<Notify>,
) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    else {
        tracing::error!(
            reason_code = "client_init_failed",
            "syslog forwarding client could not start"
        );
        return;
    };
    let mut attempt = 0u32;
    loop {
        if target.trim().is_empty() {
            set_error(&state, "no_delivery_target");
            notify.notified().await;
            continue;
        }
        let Some(request) = next_request(&state) else {
            notify.notified().await;
            continue;
        };
        let mut call = client
            .post(format!(
                "{}/v1/syslog-forward",
                target.trim_end_matches('/')
            ))
            .json(&request);
        if let Some(token) = token.as_deref() {
            call = call.bearer_auth(token);
        }
        match call.send().await {
            Ok(response) if response.status().is_success() => {
                let status = response.status();
                match response.json::<SyslogForwardResponse>().await {
                    Ok(response) => match apply_receipts(&state, &request, &response.receipts) {
                        Ok(()) => {
                            attempt = 0;
                            set_error(&state, "none");
                        }
                        Err(error) => {
                            tracing::error!(
                                reason_code = "local_spool_persist_failed",
                                error = format!("{error:#}"),
                                "syslog receipt could not be checkpointed"
                            );
                            set_error(&state, "local_spool_persist_failed");
                            let delay = retry_delay_ms(attempt, None, random_u64());
                            retry_sleep(&mut attempt, delay).await;
                        }
                    },
                    Err(error) => {
                        tracing::warn!(
                            reason_code = "invalid_receipt_response",
                            status = %status,
                            error = %error,
                            "syslog receiver response rejected"
                        );
                        set_error(&state, "invalid_receipt_response");
                        let delay = retry_delay_ms(attempt, None, random_u64());
                        retry_sleep(&mut attempt, delay).await;
                    }
                }
            }
            Ok(response) => {
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok());
                let plan = retry_plan(
                    response.status().as_u16(),
                    attempt,
                    retry_after,
                    random_u64(),
                );
                tracing::warn!(status = %response.status(), reason_code = plan.reason_code, "syslog forwarding deferred");
                set_error(&state, plan.reason_code);
                retry_sleep(&mut attempt, plan.delay_ms).await;
            }
            Err(error) => {
                tracing::warn!(
                    reason_code = "transport_unavailable",
                    error = %error,
                    "syslog forwarding deferred"
                );
                set_error(&state, "transport_unavailable");
                let delay = retry_delay_ms(attempt, None, random_u64());
                retry_sleep(&mut attempt, delay).await;
            }
        }
    }
}

fn next_request(state: &Arc<Mutex<SenderState>>) -> Option<SyslogForwardRequest> {
    let mut state = state.lock().expect("syslog sender state poisoned");
    let source_key = next_source_key(&state.spool)?;
    let mut bytes = 0usize;
    let records = state
        .spool
        .records
        .iter()
        .filter(|record| {
            source_key_of(&record.source_instance).is_some_and(|key| key == source_key)
        })
        .take_while(|record| {
            let include = bytes == 0 || bytes.saturating_add(record.line.len()) <= MAX_BATCH_BYTES;
            if include {
                bytes = bytes.saturating_add(record.line.len());
            }
            include
        })
        .take(MAX_BATCH_RECORDS)
        .cloned()
        .collect();
    let gaps = state
        .spool
        .gaps
        .iter()
        .filter(|gap| source_key_of(&gap.source_instance).is_some_and(|key| key == source_key))
        .take(50)
        .cloned()
        .collect();
    state.spool.last_dispatched_source = Some(source_key);
    Some(SyslogForwardRequest { records, gaps })
}

fn apply_receipts(
    state: &Arc<Mutex<SenderState>>,
    request: &SyslogForwardRequest,
    receipts: &[String],
) -> Result<()> {
    let outbound_keys: HashSet<&str> = request
        .records
        .iter()
        .map(|record| record.idempotency_key.as_str())
        .chain(request.gaps.iter().map(|gap| gap.idempotency_key.as_str()))
        .collect();
    let keys: HashSet<&str> = receipts.iter().map(String::as_str).collect();
    if !keys.is_subset(&outbound_keys) {
        anyhow::bail!("syslog receiver acknowledged a record outside the submitted batch");
    }
    let mut state = state.lock().expect("syslog sender state poisoned");
    state
        .spool
        .records
        .retain(|record| !keys.contains(record.idempotency_key.as_str()));
    state
        .spool
        .gaps
        .retain(|gap| !keys.contains(gap.idempotency_key.as_str()));
    save_spool(&state.spool_path, &state.spool)
}

struct RetryPlan {
    reason_code: &'static str,
    delay_ms: u64,
}

fn retry_plan(status: u16, attempt: u32, retry_after: Option<u64>, entropy: u64) -> RetryPlan {
    let reason_code = if status == 429 {
        "receiver_backpressure"
    } else if (500..=599).contains(&status) {
        "receiver_unavailable"
    } else {
        "receiver_rejected"
    };
    RetryPlan {
        reason_code,
        delay_ms: retry_delay_ms(attempt, retry_after, entropy),
    }
}

fn retry_delay_ms(attempt: u32, retry_after: Option<u64>, entropy: u64) -> u64 {
    if let Some(seconds) = retry_after {
        return seconds.saturating_mul(1_000).min(RECONNECT_MAX_MS);
    }
    let backoff = backoff_ms(attempt);
    let jitter_window = (backoff / 10).max(1);
    backoff.saturating_add(entropy % (jitter_window + 1))
}

async fn retry_sleep(attempt: &mut u32, delay: u64) {
    *attempt = attempt.saturating_add(1);
    sleep(Duration::from_millis(delay)).await;
}
fn set_error(state: &Arc<Mutex<SenderState>>, code: &'static str) {
    let mut state = state.lock().expect("syslog sender state poisoned");
    state.last_error_code = if state.recovery_required {
        Some("spool_recovery_required")
    } else {
        (code != "none").then_some(code)
    };
}
fn backoff_ms(attempt: u32) -> u64 {
    500u64
        .saturating_mul(1u64 << attempt.min(6))
        .min(RECONNECT_MAX_MS)
}
pub(super) fn delivery_key(source: &str, epoch: u64, sequence: u64, kind: &str) -> String {
    format!("{source}:{epoch}:{sequence}:{kind}")
}
fn stable_source_key(source_key: &str) -> String {
    let digest = Sha256::digest(source_key.as_bytes());
    format!("source-{}", hex::encode(&digest[..12]))
}
pub(super) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn random_source_instance() -> String {
    let mut bytes = [0; 16];
    random_fill(&mut bytes).expect("OS random source identity");
    hex::encode(bytes)
}

fn random_u64() -> u64 {
    let mut bytes = [0; 8];
    random_fill(&mut bytes).expect("OS random retry jitter");
    u64::from_le_bytes(bytes)
}
pub fn format_rfc5424(
    pri: u8,
    timestamp: &str,
    hostname: &str,
    app_name: &str,
    procid: &str,
    msg: &str,
) -> String {
    let msg = msg.replace('\n', " ");
    let app = sanitise_field(app_name, "cortex-agent");
    let proc = sanitise_field(procid, "-");
    format!("<{pri}>1 {timestamp} {hostname} {app} {proc} - - {msg}")
}
pub const PRI_LOCAL0_INFO: u8 = 16 * 8 + 6;
pub const PRI_LOCAL0_WARN: u8 = 16 * 8 + 4;
pub const PRI_LOCAL0_ERR: u8 = 16 * 8 + 3;
pub fn local0_pri(severity: u8) -> u8 {
    16 * 8 + severity.min(7)
}
fn sanitise_field<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() || value.len() > 48 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        fallback
    } else {
        value
    }
}

#[cfg(test)]
#[path = "syslog_sender_tests.rs"]
mod tests;
