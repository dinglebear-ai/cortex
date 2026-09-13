//! Durable HTTP forwarding for configured fleet-agent file tails.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};

use crate::agent_file_tail_ingest::{AgentFileTailIngestRequest, AgentFileTailRecord};

use super::syslog_file::FileTailSource;

const EOF_SLEEP_MS: u64 = 500;

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
        while let Err(error) = send_line(&client, &config, raw).await {
            tracing::warn!(error = %error, path = %config.source.path.display(), "agent file-tail delivery failed; retrying current line");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn send_line(
    client: &reqwest::Client,
    config: &FileTailForwardConfig,
    line: &str,
) -> Result<()> {
    let tag = config.source.tag.as_deref().unwrap_or("file-tail");
    let source_id = source_id(tag);
    let path_basename = config
        .source
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    let record = AgentFileTailRecord {
        hostname: config.hostname.clone(),
        source_id,
        tag: tag.to_string(),
        path_basename: path_basename.to_string(),
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        message: line.to_string(),
    };
    let url = format!("{}/v1/file-tails", config.target.trim_end_matches('/'));
    let mut request = client.post(url).json(&AgentFileTailIngestRequest {
        records: vec![record],
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
