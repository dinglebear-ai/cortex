use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::observability::RuntimeObservability;

const REMOTE_DOCKER_EVENT_RECONNECT_SECS: u64 = 10;
pub(super) const REMOTE_DOCKER_EVENTS_UNSUPPORTED_MARKER: &str =
    "cortex: remote Docker events unsupported: docker command not found";

pub(super) fn spawn_remote_docker_event_tasks(
    config: &crate::inventory::InventoryConfig,
    trigger: mpsc::Sender<()>,
    token: CancellationToken,
    observability: Arc<RuntimeObservability>,
) -> Vec<JoinHandle<()>> {
    if !remote_docker_events_enabled() {
        tracing::info!("inventory_refresh: remote Docker event streams disabled");
        return Vec::new();
    }
    let resolution =
        crate::inventory::ssh::configured_hosts(config.ssh_config.as_deref(), &config.ssh_hosts);
    for warning in &resolution.warnings {
        tracing::warn!(warning = %warning, "inventory_refresh: remote Docker event host resolution degraded");
    }
    if resolution.no_usable_explicit_hosts() {
        tracing::warn!(
            "inventory_refresh: remote Docker event streams skipped; no explicitly configured SSH hosts were usable"
        );
        return Vec::new();
    }
    let ssh_context = crate::inventory::ssh::SshContext::new(
        crate::inventory::ssh::SshOptions::from_env(config.ssh_config.as_deref())
            .with_event_stream_defaults()
            .with_max_concurrent(resolution.hosts.len().max(1))
            .expect("event stream host count is non-zero after host resolution"),
    );
    resolution
        .hosts
        .into_iter()
        .map(|host| {
            let trigger = trigger.clone();
            let token = token.clone();
            let ssh_context = ssh_context.clone();
            let observability = Arc::clone(&observability);
            tokio::spawn(async move {
                let mut failure_log = EventStreamFailureLog::default();
                loop {
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => break,
                        result = run_remote_docker_events_once(&host, &ssh_context, trigger.clone(), token.clone()) => {
                            if let Err(error) = result {
                                let error = error.to_string();
                                if remote_docker_events_unsupported(&error) {
                                    observability.record_remote_docker_event_stream_failure(&host, &error);
                                    tracing::warn!(
                                        host,
                                        error,
                                        "inventory_refresh: remote Docker event streams disabled for host; Docker is unavailable"
                                    );
                                    break;
                                }
                                observability.record_remote_docker_event_stream_failure(&host, &error);
                                failure_log.record(&host, &error);
                            }
                        }
                    }
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(REMOTE_DOCKER_EVENT_RECONNECT_SECS)) => {}
                    }
                }
            })
        })
        .collect()
}

#[derive(Default)]
pub(super) struct EventStreamFailureLog {
    pub(super) failures: u64,
}

impl EventStreamFailureLog {
    pub(super) fn record(&mut self, host: &str, error: &str) {
        self.failures = self.failures.saturating_add(1);
        if self.failures == 1 || self.failures.is_multiple_of(6) {
            tracing::warn!(
                host,
                failures = self.failures,
                error,
                "inventory_refresh: remote Docker event stream degraded"
            );
        } else {
            tracing::debug!(
                host,
                failures = self.failures,
                error,
                "inventory_refresh: remote Docker event stream still degraded"
            );
        }
    }
}

pub(super) async fn run_remote_docker_events_once(
    host: &str,
    ssh_context: &crate::inventory::ssh::SshContext,
    trigger: mpsc::Sender<()>,
    token: CancellationToken,
) -> anyhow::Result<()> {
    let Some(_permit) = ssh_context.acquire_owned_cancellable(&token).await? else {
        return Ok(());
    };
    let args = remote_docker_events_ssh_args(ssh_context, host)?;
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ssh stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("ssh stderr unavailable"))?;
    let stderr_task = tokio::spawn(async move { read_stream_sample(stderr).await });
    let mut lines = BufReader::new(stdout).lines();
    let mut stdout_sample = OutputSample::default();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                let _ = child.kill().await;
                stderr_task.abort();
                break;
            }
            line = lines.next_line() => match line? {
                Some(line) if !line.trim().is_empty() => {
                    stdout_sample.push_line(&line);
                    let _ = trigger.try_send(());
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    let status = child.wait().await?;
    let stderr_sample = stderr_task
        .await
        .unwrap_or_else(|error| format!("stderr reader task failed: {error}"));
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "ssh docker events exited with status={status}; stdout_sample={}; stderr_sample={}",
            stdout_sample.as_str(),
            stderr_sample
        ))
    }
}

#[derive(Default)]
pub(super) struct OutputSample {
    text: String,
    truncated: bool,
}

impl OutputSample {
    pub(super) fn push_line(&mut self, line: &str) {
        const MAX_SAMPLE_BYTES: usize = 4096;
        if self.text.len() >= MAX_SAMPLE_BYTES {
            self.truncated = true;
            return;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let remaining = MAX_SAMPLE_BYTES.saturating_sub(self.text.len());
        if line.len() > remaining {
            let safe_end = line
                .char_indices()
                .map(|(idx, _)| idx)
                .take_while(|idx| *idx <= remaining)
                .last()
                .unwrap_or(0);
            self.text.push_str(&line[..safe_end]);
            self.truncated = true;
        } else {
            self.text.push_str(line);
        }
    }

    pub(super) fn as_str(&self) -> String {
        if self.truncated {
            format!("{}...<truncated>", self.text)
        } else {
            self.text.clone()
        }
    }
}

pub(super) async fn read_stream_sample<R>(mut reader: R) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    const MAX_SAMPLE_BYTES: usize = 4096;
    let mut sample = Vec::new();
    let mut buf = [0u8; 1024];
    let mut truncated = false;
    loop {
        match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = MAX_SAMPLE_BYTES.saturating_sub(sample.len());
                if remaining == 0 {
                    truncated = true;
                    continue;
                }
                let take = n.min(remaining);
                sample.extend_from_slice(&buf[..take]);
                truncated |= take < n;
            }
            Err(error) => return format!("stderr read failed: {error}"),
        }
    }
    let mut text = String::from_utf8_lossy(&sample).to_string();
    if truncated {
        text.push_str("...<truncated>");
    }
    text
}

pub(super) fn remote_docker_events_enabled() -> bool {
    std::env::var("CORTEX_INVENTORY_REMOTE_DOCKER_EVENTS")
        .ok()
        .as_deref()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no"
            )
        })
        .unwrap_or(false)
}

pub(super) fn remote_docker_events_ssh_args(
    ssh_context: &crate::inventory::ssh::SshContext,
    host: &str,
) -> anyhow::Result<Vec<String>> {
    let command = format!(
        "if ! command -v docker >/dev/null 2>&1; then echo '{REMOTE_DOCKER_EVENTS_UNSUPPORTED_MARKER}' >&2; exit 78; fi; exec docker events --filter type=container --format '{{{{json .}}}}'"
    );
    ssh_context.ssh_args(host, &command)
}

pub(super) fn remote_docker_events_unsupported(error: &str) -> bool {
    error.contains(REMOTE_DOCKER_EVENTS_UNSUPPORTED_MARKER)
}
