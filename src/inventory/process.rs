use anyhow::{Result, anyhow};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::inventory::limits::MAX_COMMAND_OUTPUT_BYTES;
use crate::inventory::redaction::redact_error;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u128,
    pub truncated: bool,
}

pub async fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<CommandOutput> {
    run_command_capped(program, args, timeout, MAX_COMMAND_OUTPUT_BYTES).await
}

pub async fn run_command_capped(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<CommandOutput> {
    let start = Instant::now();
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| anyhow!("{program} spawn failed: {error}"))?;

    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stdout_task = tokio::spawn(async move { read_capped(&mut stdout, max_output_bytes).await });
    let stderr_task = tokio::spawn(async move { read_capped(&mut stderr, max_output_bytes).await });
    let wait = tokio::time::timeout(timeout, child.wait()).await;
    let status = match wait {
        Ok(Ok(status)) => Some(status.code().unwrap_or(-1)),
        Ok(Err(error)) => {
            stdout_task.abort();
            stderr_task.abort();
            return Err(anyhow!("{program} wait failed: {error}"));
        }
        Err(_) => {
            stdout_task.abort();
            stderr_task.abort();
            let _ = child.kill().await;
            return Err(anyhow!(
                "{program} timed out after {}ms",
                timeout.as_millis()
            ));
        }
    };
    let (stdout, out_truncated) = stdout_task.await??;
    let (stderr, err_truncated) = stderr_task.await??;
    let (stderr, redaction_truncated) = redact_error(stderr);
    Ok(CommandOutput {
        status,
        stdout,
        stderr,
        elapsed_ms: start.elapsed().as_millis(),
        truncated: out_truncated || err_truncated || redaction_truncated,
    })
}

async fn read_capped<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<(String, bool)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining == 0 {
            truncated = true;
            continue;
        }
        let take = n.min(remaining);
        buf.extend_from_slice(&tmp[..take]);
        truncated |= take < n;
    }
    Ok((String::from_utf8_lossy(&buf).to_string(), truncated))
}

pub fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote = None;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    token_started = true;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                token_started = true;
            }
            (None, ch) if ch.is_whitespace() => {
                if token_started {
                    words.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            (Some('\''), '\'') | (Some('"'), '"') => {
                quote = None;
            }
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    token_started = true;
                }
            }
            (_, ch) => {
                current.push(ch);
                token_started = true;
            }
        }
    }
    if token_started {
        words.push(current);
    }
    words
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
