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

const EOF_SLEEP_MS: u64 = 500;
static DELIVERY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct FileTailForwardConfig {
    pub source: FileTailSource,
    pub target: String,
    pub token: Option<String>,
    pub hostname: String,
}

pub async fn run(config: FileTailForwardConfig) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build file-tail forwarder client")?;
    let mut reader = super::tail_reader::TailReader::open(&config.source.path).await?;
    loop {
        let Some(line) = reader.next_line().await? else {
            tokio::time::sleep(Duration::from_millis(EOF_SLEEP_MS)).await;
            continue;
        };
        let raw = line.trim_end_matches(['\r', '\n']);
        if raw.is_empty() {
            continue;
        }
        let record = build_record(&config, raw);
        while let Err(error) = send_record(&client, &config, &record).await {
            tracing::warn!(error = %error, path = %config.source.path.display(), "agent file-tail delivery failed; retrying current line");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

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
    if !response.status().is_success() {
        anyhow::bail!(
            "agent file-tail forward rejected: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }
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
