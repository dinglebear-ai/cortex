//! Durable HTTP forwarding for configured fleet-agent file tails.

use std::time::Duration;
use std::{
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use crate::agent_file_tail_ingest::{AgentFileTailIngestRequest, AgentFileTailRecord};

use super::syslog_file::FileTailSource;

mod state;
pub use state::checkpoint_path;
use state::{State, Store};

const EOF_SLEEP_MS: u64 = 500;
static DELIVERY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct FileTailForwardConfig {
    pub source: FileTailSource,
    pub target: String,
    pub token: Option<String>,
    pub hostname: String,
    pub checkpoint_path: std::path::PathBuf,
}

pub async fn run(config: FileTailForwardConfig) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build file-tail forwarder client")?;
    validate_config(&config)?;
    // Keep a single owner for this bounded outbox throughout delivery.
    let (store, saved) = Store::open(&config.checkpoint_path).await?;
    let (mut reader, mut state) = if let Some(saved) = saved {
        // Deliver the persisted outbox even if the source disappeared meanwhile.
        (None, saved)
    } else {
        let mut reader = super::tail_reader::TailReader::open_durable(&config.source.path).await?;
        let state = State {
            cursor: reader.cursor().await?,
            pending: None,
        };
        (Some(reader), state)
    };
    store.save(&state).await?;
    loop {
        if let Some(record) = &state.pending {
            match send_record(&client, &config, record).await {
                Ok(()) => {
                    state.pending = None;
                    // An acknowledgement that cannot be persisted is retried with
                    // the identical saved key on restart, not silently discarded.
                    store.save(&state).await?;
                }
                Err(error) if error.downcast_ref::<PermanentRejection>().is_some() => {
                    tracing::error!(error = %error, path = %config.source.path.display(),
                        reason_code = "file_tail_delivery_operator_action_required",
                        "file-tail forwarding stopped; pending evidence retained until configuration or server rejection is repaired and agent restarted");
                    std::future::pending::<()>().await;
                }
                Err(error) => {
                    tracing::warn!(error = %error, path = %config.source.path.display(), "agent file-tail delivery failed; retrying durable current line");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
            continue;
        }
        if reader.is_none() {
            reader = Some(
                super::tail_reader::TailReader::restore(&config.source.path, state.cursor.clone())
                    .await?,
            );
        }
        let active_reader = reader.as_mut().unwrap();
        let line = active_reader.next_line().await?;
        state.cursor = active_reader.cursor().await?;
        state.pending = line
            .as_deref()
            .filter(|raw| !raw.is_empty())
            .map(|raw| build_record(&config, raw));
        // Cursor and pending evidence commit together before any network attempt.
        store.save(&state).await?;
        if state.pending.is_none() {
            tokio::time::sleep(Duration::from_millis(EOF_SLEEP_MS)).await;
        }
    }
}

fn validate_config(config: &FileTailForwardConfig) -> Result<()> {
    anyhow::ensure!(
        !config.hostname.is_empty()
            && config.hostname.len() <= 255
            && config
                .hostname
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
        "invalid file-tail hostname; operator action required"
    );
    let tag = config.source.tag.as_deref().unwrap_or("file-tail");
    anyhow::ensure!(
        !tag.trim().is_empty() && tag.len() <= 255,
        "invalid file-tail tag: expected 1..255 bytes; operator action required"
    );
    let basename = config
        .source
        .path
        .file_name()
        .and_then(|name| name.to_str());
    anyhow::ensure!(
        basename.is_some_and(|name| !name.trim().is_empty() && name.len() <= 255),
        "invalid file-tail path basename; operator action required"
    );
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("permanent file-tail rejection: {0}; operator action required")]
struct PermanentRejection(reqwest::StatusCode);

fn build_record(config: &FileTailForwardConfig, line: &str) -> AgentFileTailRecord {
    let now = Utc::now();
    let timestamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    let tag = config.source.tag.as_deref().unwrap_or("file-tail");
    let source_id = source_id(tag);
    let path_basename = config
        .source
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    let sequence = DELIVERY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let idempotency_key = delivery_key(
        config,
        &source_id,
        &timestamp,
        line,
        now.timestamp_nanos_opt().unwrap_or_default(),
        sequence,
    );
    AgentFileTailRecord {
        idempotency_key: Some(idempotency_key),
        hostname: config.hostname.clone(),
        source_id,
        tag: tag.to_string(),
        path_basename: path_basename.to_string(),
        timestamp,
        message: line.to_string(),
    }
}

fn delivery_key(
    config: &FileTailForwardConfig,
    source_id: &str,
    timestamp: &str,
    line: &str,
    created_at_nanos: i64,
    sequence: u64,
) -> String {
    let digest = Sha256::digest(format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}",
        process::id(),
        created_at_nanos,
        sequence,
        config.hostname,
        source_id,
        timestamp,
        line
    ));
    hex::encode(digest)
}

async fn send_record(
    client: &reqwest::Client,
    config: &FileTailForwardConfig,
    record: &AgentFileTailRecord,
) -> Result<()> {
    let url = format!("{}/v1/file-tails", config.target.trim_end_matches('/'));
    let mut request = client.post(url).json(&AgentFileTailIngestRequest {
        records: vec![record.clone()],
    });
    if let Some(token) = &config.token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .context("agent file-tail POST failed")?;
    let status = response.status();
    if status.is_client_error()
        && status != reqwest::StatusCode::REQUEST_TIMEOUT
        && status != reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(PermanentRejection(status).into());
    }
    if !status.is_success() {
        anyhow::bail!("agent file-tail forward rejected: {status}");
    }
    // The endpoint counts newly inserted rows; zero is a durable duplicate.
    let receipt: serde_json::Value = response
        .json()
        .await
        .context("file-tail response was not a durable receipt")?;
    anyhow::ensure!(
        receipt
            .get("accepted")
            .and_then(|accepted| accepted.as_u64())
            .is_some_and(|accepted| accepted <= 1),
        "file-tail returned incomplete durable acknowledgement"
    );
    Ok(())
}

fn source_id(tag: &str) -> String {
    let normalized: String = tag
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        "file-tail".to_string()
    } else {
        trimmed.chars().take(255).collect()
    }
}

#[cfg(test)]
#[path = "file_tail_tests.rs"]
mod tests;
