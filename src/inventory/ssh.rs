use anyhow::{Result, anyhow};
use futures_util::future::BoxFuture;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::inventory::limits::MAX_COMMAND_OUTPUT_BYTES;
use crate::inventory::process::{CommandOutput, run_command_capped};

const SSH_IGNORE_UNKNOWN_OPTIONS: &str = "IgnoreUnknown=WarnWeakCrypto";
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 4;
// Inventory commands can legitimately pause while a busy host schedules the
// remote shell or walks config trees. Three seconds with one missed reply
// turned transient I/O pressure into false host failures long before the
// collector's own bounded timeout elapsed.
const DEFAULT_SERVER_ALIVE_INTERVAL_SECS: u64 = 10;
const DEFAULT_SERVER_ALIVE_COUNT_MAX: u64 = 3;
pub const DEFAULT_MAX_CONCURRENT_SSH: usize = 8;
const DEFAULT_RETRY_ATTEMPTS: usize = 1;
const DEFAULT_RETRY_INITIAL_BACKOFF_MS: u64 = 250;

type SshRunner = Arc<
    dyn Fn(Vec<String>, Duration, usize) -> BoxFuture<'static, Result<CommandOutput>> + Send + Sync,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshHostKeyPolicy {
    Strict,
    AcceptNew,
}

#[derive(Clone)]
pub struct SshOptions {
    config: Option<PathBuf>,
    known_hosts: Option<PathBuf>,
    host_key_policy: SshHostKeyPolicy,
    connect_timeout_secs: u64,
    server_alive_interval_secs: u64,
    server_alive_count_max: u64,
    max_concurrent: usize,
    retry_attempts: usize,
    retry_initial_backoff: Duration,
}

impl Default for SshOptions {
    fn default() -> Self {
        Self {
            config: None,
            known_hosts: None,
            host_key_policy: SshHostKeyPolicy::Strict,
            connect_timeout_secs: DEFAULT_CONNECT_TIMEOUT_SECS,
            server_alive_interval_secs: DEFAULT_SERVER_ALIVE_INTERVAL_SECS,
            server_alive_count_max: DEFAULT_SERVER_ALIVE_COUNT_MAX,
            max_concurrent: DEFAULT_MAX_CONCURRENT_SSH,
            retry_attempts: DEFAULT_RETRY_ATTEMPTS,
            retry_initial_backoff: Duration::from_millis(DEFAULT_RETRY_INITIAL_BACKOFF_MS),
        }
    }
}

impl SshOptions {
    pub fn for_config(config: Option<&Path>) -> Self {
        Self {
            config: config.map(Path::to_path_buf),
            ..Self::default()
        }
    }

    pub fn from_env(config: Option<&Path>) -> Self {
        let mut options = Self::for_config(config);
        options.known_hosts = env_path("CORTEX_INVENTORY_SSH_KNOWN_HOSTS");
        if env_bool("CORTEX_INVENTORY_SSH_TRUST_ON_FIRST_USE").unwrap_or(false) {
            options.host_key_policy = SshHostKeyPolicy::AcceptNew;
        }
        if let Some(max) = env_usize("CORTEX_INVENTORY_SSH_MAX_CONCURRENT").filter(|v| *v > 0) {
            options.max_concurrent = max;
        }
        if let Some(attempts) = env_usize("CORTEX_INVENTORY_SSH_RETRY_ATTEMPTS").filter(|v| *v > 0)
        {
            options.retry_attempts = attempts;
        }
        if let Some(backoff_ms) =
            env_u64("CORTEX_INVENTORY_SSH_RETRY_INITIAL_BACKOFF_MS").filter(|v| *v > 0)
        {
            options.retry_initial_backoff = Duration::from_millis(backoff_ms);
        }
        options
    }

    pub fn with_known_hosts(mut self, known_hosts: Option<PathBuf>) -> Self {
        self.known_hosts = known_hosts;
        self
    }

    pub fn with_host_key_policy(mut self, policy: SshHostKeyPolicy) -> Self {
        self.host_key_policy = policy;
        self
    }

    pub fn with_connect_timeout_secs(mut self, secs: u64) -> Result<Self> {
        if secs == 0 {
            anyhow::bail!("ssh connect timeout must be greater than zero");
        }
        self.connect_timeout_secs = secs;
        Ok(self)
    }

    pub fn with_server_alive(mut self, interval_secs: u64, count_max: u64) -> Result<Self> {
        if interval_secs == 0 {
            anyhow::bail!("ssh server alive interval must be greater than zero");
        }
        if count_max == 0 {
            anyhow::bail!("ssh server alive count must be greater than zero");
        }
        self.server_alive_interval_secs = interval_secs;
        self.server_alive_count_max = count_max;
        Ok(self)
    }

    pub fn with_max_concurrent(mut self, max_concurrent: usize) -> Result<Self> {
        if max_concurrent == 0 {
            anyhow::bail!("ssh max_concurrent must be greater than zero");
        }
        self.max_concurrent = max_concurrent;
        Ok(self)
    }

    pub fn with_retry_attempts(mut self, retry_attempts: usize) -> Result<Self> {
        if retry_attempts == 0 {
            anyhow::bail!("ssh retry_attempts must be greater than zero");
        }
        self.retry_attempts = retry_attempts;
        Ok(self)
    }

    pub fn with_retry_initial_backoff(mut self, backoff: Duration) -> Result<Self> {
        if backoff.is_zero() {
            anyhow::bail!("ssh retry initial backoff must be greater than zero");
        }
        self.retry_initial_backoff = backoff;
        Ok(self)
    }

    pub fn with_event_stream_defaults(mut self) -> Self {
        self.server_alive_interval_secs = 15;
        self.server_alive_count_max = 2;
        self
    }

    fn ssh_args(&self, host: &str, remote_command: &str) -> Result<Vec<String>> {
        if !is_safe_ssh_host(host) {
            anyhow::bail!("unsafe ssh host: {host}");
        }
        let mut args = Vec::new();
        args.push("-o".to_string());
        args.push(SSH_IGNORE_UNKNOWN_OPTIONS.to_string());
        if let Some(config) = &self.config {
            args.push("-F".to_string());
            args.push(config.display().to_string());
        }
        args.extend(["-o".to_string(), "BatchMode=yes".to_string()]);
        args.push("-o".to_string());
        args.push(match self.host_key_policy {
            SshHostKeyPolicy::Strict => "StrictHostKeyChecking=yes".to_string(),
            SshHostKeyPolicy::AcceptNew => "StrictHostKeyChecking=accept-new".to_string(),
        });
        if let Some(known_hosts) = &self.known_hosts {
            args.push("-o".to_string());
            args.push(format!("UserKnownHostsFile={}", known_hosts.display()));
        }
        args.extend([
            "-o".to_string(),
            format!("ConnectTimeout={}", self.connect_timeout_secs),
            "-o".to_string(),
            format!("ServerAliveInterval={}", self.server_alive_interval_secs),
            "-o".to_string(),
            format!("ServerAliveCountMax={}", self.server_alive_count_max),
            "--".to_string(),
            host.to_string(),
            remote_command.to_string(),
        ]);
        Ok(args)
    }
}

#[derive(Clone)]
pub struct SshContext {
    options: Arc<SshOptions>,
    limiter: Arc<Semaphore>,
    runner: SshRunner,
}

impl SshContext {
    pub fn new(options: SshOptions) -> Self {
        let runner: SshRunner = Arc::new(|args, timeout, max_output_bytes| {
            Box::pin(async move {
                let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
                run_command_capped("ssh", &refs, timeout, max_output_bytes).await
            })
        });
        Self::with_runner(options, runner)
    }

    fn with_runner(options: SshOptions, runner: SshRunner) -> Self {
        let max_concurrent = options.max_concurrent.max(1);
        Self {
            options: Arc::new(options),
            limiter: Arc::new(Semaphore::new(max_concurrent)),
            runner,
        }
    }

    #[cfg(test)]
    pub fn with_runner_for_test<F>(options: SshOptions, runner: F) -> Self
    where
        F: Fn(Vec<String>, Duration, usize) -> BoxFuture<'static, Result<CommandOutput>>
            + Send
            + Sync
            + 'static,
    {
        Self::with_runner(options, Arc::new(runner))
    }

    pub async fn run(
        &self,
        host: &str,
        remote_command: &str,
        timeout: Duration,
    ) -> Result<CommandOutput> {
        self.run_capped(host, remote_command, timeout, MAX_COMMAND_OUTPUT_BYTES)
            .await
    }

    pub async fn run_capped(
        &self,
        host: &str,
        remote_command: &str,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<CommandOutput> {
        let args = self.options.ssh_args(host, remote_command)?;
        let attempts = self.options.retry_attempts.max(1);
        let mut last_error = None;
        for attempt in 0..attempts {
            let _permit = self
                .limiter
                .acquire()
                .await
                .map_err(|_| anyhow!("ssh concurrency limiter closed"))?;
            match (self.runner)(args.clone(), timeout, max_output_bytes).await {
                Ok(output) => return Ok(output),
                Err(error) => last_error = Some(error),
            }
            drop(_permit);
            if attempt + 1 < attempts {
                tokio::time::sleep(backoff_delay_for_host(
                    host,
                    self.options.retry_initial_backoff,
                    attempt,
                ))
                .await;
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("ssh command failed without an error")))
    }

    /// Build a safe `ssh` argv for long-lived streaming processes.
    ///
    /// Prefer [`SshContext::run`] for bounded commands. Deploy streaming and
    /// Docker event streaming still need direct argv access so callers can wire
    /// stdin/stdout and cancellation around the child process.
    pub fn ssh_args(&self, host: &str, remote_command: &str) -> Result<Vec<String>> {
        self.options.ssh_args(host, remote_command)
    }

    pub async fn acquire_owned(&self) -> Result<OwnedSemaphorePermit> {
        Arc::clone(&self.limiter)
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("ssh concurrency limiter closed"))
    }

    pub async fn acquire_owned_cancellable(
        &self,
        token: &CancellationToken,
    ) -> Result<Option<OwnedSemaphorePermit>> {
        tokio::select! {
            biased;
            _ = token.cancelled() => Ok(None),
            permit = Arc::clone(&self.limiter).acquire_owned() => permit
                .map(Some)
                .map_err(closed_limiter_error),
        }
    }
}

fn closed_limiter_error(_: AcquireError) -> anyhow::Error {
    anyhow!("ssh concurrency limiter closed")
}

fn backoff_delay_for_host(host: &str, initial: Duration, attempt: usize) -> Duration {
    backoff_delay(initial, attempt).saturating_add(retry_jitter(host, initial, attempt))
}

fn backoff_delay(initial: Duration, attempt: usize) -> Duration {
    let multiplier = 1u32.checked_shl(attempt as u32).unwrap_or(u32::MAX);
    initial.saturating_mul(multiplier)
}

fn retry_jitter(host: &str, initial: Duration, attempt: usize) -> Duration {
    let max_jitter_ms = (initial.as_millis() / 2).max(1).min(u128::from(u64::MAX)) as u64;
    let mut hasher = DefaultHasher::new();
    host.hash(&mut hasher);
    attempt.hash(&mut hasher);
    Duration::from_millis(hasher.finish() % (max_jitter_ms + 1))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .as_deref()
        .map(str::trim)
        .and_then(|value| value.parse().ok())
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .as_deref()
        .map(str::trim)
        .and_then(|value| value.parse().ok())
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostResolution {
    pub hosts: Vec<String>,
    pub warnings: Vec<String>,
    pub explicit_hosts_configured: bool,
}

impl HostResolution {
    pub fn no_usable_explicit_hosts(&self) -> bool {
        self.explicit_hosts_configured && self.hosts.is_empty()
    }
}

pub fn configured_hosts(ssh_config: Option<&Path>, configured_hosts: &[String]) -> HostResolution {
    if !configured_hosts.is_empty() {
        let mut hosts = Vec::new();
        let mut warnings = Vec::new();
        for host in configured_hosts {
            if is_safe_ssh_host(host) {
                hosts.push(host.clone());
            } else {
                warnings.push(format!("rejected unsafe configured SSH host `{host}`"));
            }
        }
        if hosts.is_empty() {
            warnings.push("all explicitly configured SSH hosts were rejected".to_string());
        }
        return HostResolution {
            hosts,
            warnings,
            explicit_hosts_configured: true,
        };
    }
    let Some(path) = ssh_config else {
        return HostResolution {
            hosts: Vec::new(),
            warnings: Vec::new(),
            explicit_hosts_configured: false,
        };
    };
    match std::fs::read_to_string(path) {
        Ok(body) => {
            let (hosts, warnings) = parse_ssh_hosts_with_warnings(&body);
            HostResolution {
                hosts,
                warnings,
                explicit_hosts_configured: false,
            }
        }
        Err(error) => HostResolution {
            hosts: Vec::new(),
            warnings: vec![format!(
                "ssh config `{}` could not be read: {error}",
                path.display()
            )],
            explicit_hosts_configured: false,
        },
    }
}

/// Reject SSH host strings that could be interpreted as options or cause arg splitting.
pub fn is_safe_ssh_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && !host.chars().any(char::is_whitespace)
        && host.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '@' | '[' | ']')
        })
}

fn parse_ssh_hosts_with_warnings(body: &str) -> (Vec<String>, Vec<String>) {
    let mut hosts = Vec::new();
    let mut warnings = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed
            .strip_prefix("Host ")
            .or_else(|| trimmed.strip_prefix("host "))
        else {
            continue;
        };
        for host in rest.split_whitespace() {
            if host.contains('*') || host.contains('?') || host.eq_ignore_ascii_case("github.com") {
                continue;
            }
            if !is_safe_ssh_host(host) {
                warnings.push(format!("rejected unsafe SSH config host `{host}`"));
                continue;
            }
            if hosts.iter().any(|existing| existing == host) {
                continue;
            }
            hosts.push(host.to_string());
        }
    }
    (hosts, warnings)
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
