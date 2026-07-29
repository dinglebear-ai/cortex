//! `OsAdapter` trait — isolates `journalctl`, `systemctl`, and other OS-level
//! shell-outs from the service layer.
//!
//! # Motivation (Arch-C2)
//!
//! `src/app/service.rs` contained inline `tokio::process::Command` calls for
//! `journalctl` and `sqlite3`. These shell-outs are wrong for a service layer:
//! - They are untestable without a real system journal.
//! - They couple business logic to system-dependent binary paths.
//! - They make unit tests slow (process spawn overhead).
//!
//! Extracting behind an `Arc<dyn OsAdapter + Send + Sync>` lets tests inject a
//! `MockOsAdapter` that returns canned output without spawning processes.
//!
//! # Current status
//!
//! `CortexService` carries an `os: Arc<dyn OsAdapter + Send + Sync>` field.
//! The service layer calls `self.os.run_command(program, args).await` instead
//! of the old module-level `command_output(program, args)`. Production code
//! uses `SystemOsAdapter`; tests can supply a `MockOsAdapter`.
//!
//! Splitting `CortexService` into sub-services (`LogQueryService`,
//! `AiAnalyticsService`, etc.) is deferred — the OsAdapter extraction is the
//! prerequisite and is delivered here.

use std::path::PathBuf;
use std::time::Duration;

use tokio::process::Command;

use super::{ServiceError, ServiceResult};

// ---------------------------------------------------------------------------
// Trait

/// Abstracts OS-level operations (process execution, filesystem probes) used
/// by the service layer.
///
/// # Rules
/// - Implementors MUST be `Send + Sync` (the trait is stored in an `Arc`).
/// - Do not embed business logic here. `OsAdapter` is a thin I/O boundary,
///   not a service.
pub trait OsAdapter: Send + Sync {
    /// Run `program` with `args` and return its stdout as a string.
    ///
    /// Implementations must apply a reasonable execution timeout. The
    /// default `SystemOsAdapter` uses [`COMMAND_TIMEOUT`].
    fn run_command<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ServiceResult<String>> + Send + 'a>>;

    /// Run `program` with `args` and return the raw [`std::process::Output`].
    ///
    /// Unlike [`run_command`], a non-zero exit code is **not** an error —
    /// the caller inspects `output.status` and `output.stdout` directly.
    /// Use this for commands like `systemctl is-active` that write meaningful
    /// output to stdout even when they exit non-zero.
    ///
    /// Implementations must apply a reasonable execution timeout.
    fn probe_command<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ServiceResult<std::process::Output>> + Send + 'a>,
    >;
}

// ---------------------------------------------------------------------------
// Production implementation

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Production `OsAdapter` that delegates to real system processes.
pub struct SystemOsAdapter;

impl OsAdapter for SystemOsAdapter {
    fn run_command<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ServiceResult<String>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut command = Command::new(program);
            command.args(args).kill_on_drop(true);
            apply_dbus_env(&mut command);

            let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
                .await
                .map_err(|_| {
                    ServiceError::Internal(anyhow::anyhow!(
                        "{} {} timed out after {}s",
                        program,
                        args.join(" "),
                        COMMAND_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(anyhow::Error::from)?;

            if !output.status.success() {
                return Err(ServiceError::Internal(anyhow::anyhow!(
                    "{} {} failed: {}",
                    program,
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }

            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        })
    }

    fn probe_command<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ServiceResult<std::process::Output>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut command = Command::new(program);
            command.args(args).kill_on_drop(true);
            apply_dbus_env(&mut command);

            tokio::time::timeout(COMMAND_TIMEOUT, command.output())
                .await
                .map_err(|_| {
                    ServiceError::Internal(anyhow::anyhow!(
                        "{} {} timed out after {}s",
                        program,
                        args.join(" "),
                        COMMAND_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(anyhow::Error::from)
                .map_err(ServiceError::Internal)
        })
    }
}

/// Inject D-Bus session env vars into `command` so that `journalctl --user`
/// and `systemctl --user` can reach the D-Bus session.
///
/// Policy: inject only when `DBUS_SESSION_BUS_ADDRESS` is absent from the
/// process environment. `XDG_RUNTIME_DIR` is always set when the bus address
/// is inferred — it is not independently guarded.
fn apply_dbus_env(command: &mut Command) {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
        && let Some((runtime_dir, bus_address)) = inferred_user_bus_env()
    {
        command
            .env("XDG_RUNTIME_DIR", runtime_dir)
            .env("DBUS_SESSION_BUS_ADDRESS", bus_address);
    }
}

/// Infer the D-Bus user session socket path from the XDG runtime directory
/// when `DBUS_SESSION_BUS_ADDRESS` is not set in the environment. Required
/// for `journalctl --user` and `systemctl --user` under systemd without a
/// desktop session.
pub(crate) fn inferred_user_bus_env() -> Option<(PathBuf, String)> {
    let runtime_dir = PathBuf::from(format!("/run/user/{}", current_uid()));
    user_bus_env_for_runtime_dir(&runtime_dir)
}

fn user_bus_env_for_runtime_dir(runtime_dir: &std::path::Path) -> Option<(PathBuf, String)> {
    let bus = runtime_dir.join("bus");
    bus.exists().then(|| {
        (
            runtime_dir.to_path_buf(),
            format!("unix:path={}", bus.display()),
        )
    })
}

pub(crate) fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
#[path = "os_adapter_tests.rs"]
mod tests;
