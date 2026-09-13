use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::syslog_sender::{SyslogSender, format_rfc5424, local0_pri};

/// Read from journald via `journalctl -f -o json` and forward entries as
/// RFC 5424 syslog to the given sender.  Unix-only.
#[cfg(unix)]
pub async fn run_journald_forwarder(hostname: &str, sender: Arc<SyslogSender>) -> Result<()> {
    let child = crate::env::tokio_command("journalctl")
        .args(["-f", "-o", "json", "--no-pager"])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawn journalctl")?;

    forward_child(child, hostname, sender).await
}

#[cfg(unix)]
async fn forward_child(
    mut child: tokio::process::Child,
    hostname: &str,
    sender: Arc<SyslogSender>,
) -> Result<()> {
    let stdout = child.stdout.take().context("journalctl stdout")?;
    let mut lines = BufReader::new(stdout).lines();

    let forwarded: Result<()> = async {
        while let Some(line) = lines.next_line().await? {
            if let Some(syslog_line) = parse_entry(hostname, &line) {
                sender.send_from("journald", syslog_line).await?;
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = forwarded {
        // Reap before the supervisor starts another follower. kill_on_drop also
        // covers cancellation while either read or delivery is pending.
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(error);
    }
    let status = child.wait().await.context("wait for journalctl")?;
    anyhow::bail!("journalctl stream ended unexpectedly: {status}")
}

#[cfg(not(unix))]
pub async fn run_journald_forwarder(_hostname: &str, _sender: Arc<SyslogSender>) -> Result<()> {
    anyhow::bail!("journald is not available on this platform")
}

fn parse_entry(hostname: &str, line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;

    let msg = v.get("MESSAGE")?.as_str()?.trim();
    if msg.is_empty() {
        return None;
    }

    let priority: u8 = v
        .get("PRIORITY")
        .and_then(|p| p.as_str())
        .and_then(|p| p.parse().ok())
        .unwrap_or(6); // default info

    let app_name = v
        .get("SYSLOG_IDENTIFIER")
        .or_else(|| v.get("_SYSTEMD_UNIT"))
        .and_then(|v| v.as_str())
        .unwrap_or("journald");

    let procid = v.get("_PID").and_then(|p| p.as_str()).unwrap_or("-");

    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    Some(format_rfc5424(
        local0_pri(priority),
        &ts,
        hostname,
        app_name,
        procid,
        msg,
    ))
}

#[cfg(test)]
#[path = "journald_tests.rs"]
mod tests;
