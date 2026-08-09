//! Agent self-update — keeps the host agent binary in lockstep with the cortex
//! server it reports to.
//!
//! The server advertises its own version + a download directive in the heartbeat
//! `202` response (see `heartbeat.rs::AgentUpdateDirective`). When the agent's
//! compiled version differs from the server's, [`maybe_update`] downloads the
//! server's binary over the same bearer-authenticated channel, verifies its
//! SHA-256, sanity-checks that it runs and self-reports the expected version,
//! atomically swaps it into place (keeping a `.bak`), and re-execs.
//!
//! Safety model (matches the approved design):
//! - **integrity**: SHA-256 from the authenticated heartbeat response, verified
//!   against the downloaded bytes before anything is written over the live path.
//! - **pre-swap validation**: the freshly downloaded binary must execute
//!   `--version` and report the advertised version, so a corrupt or incompatible
//!   download is never installed.
//! - **bounded rollback**: a marker file records the in-flight update. The new
//!   process clears it after its first successful heartbeat. If `MAX_ATTEMPTS`
//!   restarts elapse without that confirmation, [`confirm_or_rollback`] restores
//!   the `.bak` and re-execs the previous binary.
//!
//! Gated by `CORTEX_AGENT_AUTO_UPDATE` (default on) in the caller.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::timeout;

/// Number of agent restarts allowed without a confirming heartbeat before the
/// in-flight update is rolled back to the previous `.bak` binary.
const MAX_ATTEMPTS: u32 = 3;

pub const SELF_UPDATE_DEADLINE: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_AGENT_BINARY_BYTES: usize = 128 * 1024 * 1024;
const MARKER_FILE: &str = ".cortex-update-state.json";

/// Server-issued update directive, deserialized from the heartbeat `202` body.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentUpdateDirective {
    /// Target version the agent should converge to (the server's own version).
    pub version: String,
    /// Path on the server to download the matching binary from, resolved
    /// relative to the agent's configured heartbeat target.
    pub path: String,
    /// Lowercase hex SHA-256 of the target binary.
    pub sha256: String,
}

/// Persisted record of an in-flight update, used for bounded auto-rollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdateMarker {
    /// Version we attempted to install (matches the running binary on success).
    target: String,
    /// Absolute path of the backed-up previous binary to roll back to.
    bak: PathBuf,
    /// Restarts observed since the swap without a confirming heartbeat.
    attempts: u32,
}

fn marker_path(exe: &Path) -> PathBuf {
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(MARKER_FILE)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Join a base URL with a server-supplied relative path.
fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// True when the directive asks for a version different from the one compiled
/// into this binary. The server is the source of truth, so any difference
/// (upgrade or downgrade) converges the agent toward the server.
pub fn update_needed(directive: &AgentUpdateDirective) -> bool {
    directive.version != env!("CARGO_PKG_VERSION")
}

pub fn build_update_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(CHUNK_IDLE_TIMEOUT)
        .timeout(SELF_UPDATE_DEADLINE)
        .build()
        .context("build agent self-update HTTP client")
}

async fn download_binary(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    max_bytes: usize,
    first_byte_timeout: Duration,
    chunk_idle_timeout: Duration,
) -> Result<Vec<u8>> {
    let mut request = client.get(url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .context("agent binary download failed")?
        .error_for_status()
        .context("agent binary download returned error status")?;

    if let Some(content_length) = response.content_length()
        && content_length > max_bytes as u64
    {
        bail!("agent binary content length {content_length} exceeds {max_bytes}-byte limit");
    }

    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(max_bytes);
    let mut bytes = Vec::with_capacity(initial_capacity);
    let mut stream = response.bytes_stream();
    let mut first_chunk = true;

    loop {
        let deadline = if first_chunk {
            first_byte_timeout
        } else {
            chunk_idle_timeout
        };
        let next = timeout(deadline, stream.next()).await.map_err(|_| {
            if first_chunk {
                anyhow!("agent binary first byte exceeded {deadline:?}")
            } else {
                anyhow!("agent binary download stalled for {deadline:?}")
            }
        })?;

        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.context("read agent binary body")?;
        let new_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| anyhow!("agent binary size overflow"))?;
        if new_len > max_bytes {
            bail!("agent binary exceeded {max_bytes}-byte limit while streaming");
        }
        bytes.extend_from_slice(&chunk);
        first_chunk = false;
    }

    Ok(bytes)
}

/// Download, verify, install, and re-exec the advertised binary. On success this
/// function does not return (the process image is replaced). It returns `Ok(())`
/// only when no update was needed; any failure leaves the live binary untouched.
pub async fn maybe_update(
    client: &reqwest::Client,
    target_base: &str,
    token: Option<&str>,
    directive: &AgentUpdateDirective,
) -> Result<()> {
    if !update_needed(directive) {
        return Ok(());
    }

    let current = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe().context("resolve current_exe")?;
    ensure_binary_still_present(&exe)?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("current_exe has no parent dir"))?
        .to_path_buf();

    tracing::warn!(
        from = current,
        to = %directive.version,
        "agent behind server; downloading update"
    );

    // 1. Download over the authenticated channel with explicit size and idle limits.
    let url = join_url(target_base, &directive.path);
    let bytes = download_binary(
        client,
        &url,
        token,
        MAX_AGENT_BINARY_BYTES,
        FIRST_BYTE_TIMEOUT,
        CHUNK_IDLE_TIMEOUT,
    )
    .await?;

    // 2. Verify integrity before touching disk near the live path.
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(&directive.sha256) {
        bail!(
            "agent binary sha256 mismatch: expected {}, got {got}",
            directive.sha256
        );
    }

    // 3. Stage into the same directory (atomic rename requires same filesystem).
    let tmp = dir.join(format!(".cortex-update-{}.tmp", directive.version));
    std::fs::write(&tmp, &bytes).with_context(|| format!("write staged binary {tmp:?}"))?;
    set_executable(&tmp)?;

    // 4. Pre-swap validation: the new binary must run and self-report the
    //    advertised version. Guards against corrupt/incompatible downloads.
    if let Err(error) = validate_binary(&tmp, &directive.version) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error.context("staged agent binary failed validation; not installed"));
    }

    // 5. Keep a rollback copy of the current binary, then atomically swap.
    let bak = backup_current_binary(&exe, &dir, current)?;

    // Record the in-flight update before the swap so a boot-crash is recoverable.
    write_marker(
        &exe,
        &UpdateMarker {
            target: directive.version.clone(),
            bak: bak.clone(),
            attempts: 0,
        },
    )?;

    std::fs::rename(&tmp, &exe).with_context(|| format!("swap new binary into {exe:?}"))?;

    tracing::warn!(
        from = current,
        to = %directive.version,
        "agent binary updated; re-executing"
    );
    reexec(&exe)
}

/// On Linux, `current_exe()` resolves through `/proc/self/exe`. If the file at
/// that path was replaced out from under the running process (e.g. a
/// concurrent rebuild rewrote the exact path this process was exec'd from),
/// the kernel appends " (deleted)" to the resolved path once the original
/// dentry is unlinked, and any filesystem op against it fails with ENOENT.
/// Detect that up front and bail with a clear diagnosis instead of a cryptic
/// ENOENT three steps later out of `backup_current_binary`.
fn ensure_binary_still_present(exe: &Path) -> Result<()> {
    if !exe.exists() {
        bail!(
            "current binary no longer exists at {exe:?} (likely replaced by something \
             other than self-update, e.g. a concurrent rebuild); skipping this update cycle"
        );
    }
    Ok(())
}

fn backup_current_binary(exe: &Path, dir: &Path, current: &str) -> Result<PathBuf> {
    let base = unique_backup_path_base(dir, current);
    for attempt in 0..100 {
        let bak = if attempt == 0 {
            base.clone()
        } else {
            dir.join(format!(
                "{}-{attempt}",
                base.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("cortex.bak")
            ))
        };
        match std::fs::hard_link(exe, &bak).or_else(|_| std::fs::copy(exe, &bak).map(|_| ())) {
            Ok(()) => return Ok(bak),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("back up current binary to {bak:?}"));
            }
        }
    }
    bail!("could not allocate a unique backup path for agent self-update")
}

fn unique_backup_path_base(dir: &Path, current: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    dir.join(format!("cortex.bak-{current}-{pid}-{now}"))
}

/// Confirm a just-installed update is healthy, or roll back if it never settled.
///
/// Called once at agent startup, before the heartbeat loop. If a marker shows
/// the running binary matches the intended target, it bumps the restart counter;
/// once `MAX_ATTEMPTS` restarts pass without [`confirm_update_success`] clearing
/// the marker (i.e. no successful heartbeat), it restores the `.bak` and re-execs.
pub fn confirm_or_rollback() -> Result<()> {
    let exe = std::env::current_exe().context("resolve current_exe")?;
    let path = marker_path(&exe);
    let Some(mut marker) = read_marker(&path) else {
        return Ok(());
    };

    let current = env!("CARGO_PKG_VERSION");
    if marker.target != current {
        // Running binary is not the one the marker tracked (e.g. manual change);
        // the marker is stale — drop it without acting.
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }

    if marker.attempts >= MAX_ATTEMPTS {
        tracing::error!(
            target = %marker.target,
            attempts = marker.attempts,
            "agent update never confirmed healthy; rolling back"
        );
        if marker.bak.exists() {
            std::fs::rename(&marker.bak, &exe)
                .with_context(|| format!("restore rollback binary from {:?}", marker.bak))?;
            let _ = std::fs::remove_file(&path);
            return reexec(&exe);
        }
        // No backup to restore — give up on rollback but clear the marker so we
        // stop looping; the operator must intervene.
        let _ = std::fs::remove_file(&path);
        bail!(
            "agent update unhealthy but no rollback binary at {:?}",
            marker.bak
        );
    }

    marker.attempts += 1;
    write_marker(&exe, &marker)?;
    Ok(())
}

/// Clear the in-flight update marker after the agent's first successful
/// heartbeat, finalizing the update. Also prunes the retained `.bak`.
pub fn confirm_update_success() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path = marker_path(&exe);
    if let Some(marker) = read_marker(&path) {
        tracing::info!(version = %marker.target, "agent update confirmed healthy");
        let _ = std::fs::remove_file(&marker.bak);
        let _ = std::fs::remove_file(&path);
    }
}

fn validate_binary(path: &Path, expected_version: &str) -> Result<()> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .context("run --version on staged binary")?;
    if !output.status.success() {
        bail!("staged binary --version exited with {}", output.status);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected_version) {
        bail!(
            "staged binary reported '{}', expected version {expected_version}",
            stdout.trim()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).context("chmod staged binary")
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn reexec(exe: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // `exec` only returns on failure.
    let error = Command::new(exe).args(args).exec();
    Err(anyhow!("re-exec of {exe:?} failed: {error}"))
}

#[cfg(not(unix))]
fn reexec(_exe: &Path) -> Result<()> {
    bail!("agent re-exec is only supported on unix")
}

fn write_marker(exe: &Path, marker: &UpdateMarker) -> Result<()> {
    let path = marker_path(exe);
    let json = serde_json::to_string(marker).context("serialize update marker")?;
    std::fs::write(&path, json).with_context(|| format!("write update marker {path:?}"))
}

fn read_marker(path: &Path) -> Option<UpdateMarker> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
#[path = "self_update_tests.rs"]
mod tests;
