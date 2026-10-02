use std::io::{self, Write as _};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use crate::setup::{PhaseTimer, SetupPhase, SetupStatus};

#[derive(Debug, Clone)]
pub(super) struct RemoteOutput {
    pub status_success: bool,
    /// The remote exit code; `None` when the ssh process was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub(super) trait RemoteRunner {
    fn latest_stable_version(&mut self) -> io::Result<String> {
        super::release::latest_stable_version()
    }

    fn run(&mut self, host: &str, script: &str, stdin: Option<&str>) -> io::Result<RemoteOutput>;
}

pub(super) struct SshRemoteRunner;

impl RemoteRunner for SshRemoteRunner {
    fn run(&mut self, host: &str, script: &str, stdin: Option<&str>) -> io::Result<RemoteOutput> {
        let args = crate::inventory::ssh::SshContext::new(
            crate::inventory::ssh::SshOptions::from_env(None),
        )
        .ssh_args(host, "sh -s")
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let mut command = crate::env::command("ssh");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        feed_and_reap(host, child, script, stdin)
    }
}

pub(super) struct LocalRemoteRunner;

impl RemoteRunner for LocalRemoteRunner {
    fn run(&mut self, host: &str, script: &str, stdin: Option<&str>) -> io::Result<RemoteOutput> {
        let mut command = crate::env::command("sh");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .args(["-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        feed_and_reap(host, child, script, stdin)
    }
}

/// Pipe `script` (then `stdin`) into the remote `sh -s`, then *always* close
/// stdin and reap the child before deciding.
///
/// A remote that exits early (auth failure, missing shell, refused command)
/// breaks the pipe mid-write. Returning that `BrokenPipe` straight away would
/// leak the unreaped child and discard its stderr, reporting the symptom
/// ("Broken pipe (os error 32)") instead of the cause ssh printed. So the exit
/// status and captured output win; the write error only surfaces when the
/// remote exited 0 before consuming all of its piped input (the script either
/// never started or exited early), so the run cannot be trusted as a success.
/// The write runs on its own thread while stdout/stderr drain, so a
/// remote that fills its output pipe before reading the rest of the script
/// cannot deadlock against us.
fn feed_and_reap(
    host: &str,
    mut child: Child,
    script: &str,
    stdin: Option<&str>,
) -> io::Result<RemoteOutput> {
    let Some(mut child_stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "failed to open ssh stdin",
        ));
    };
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("deployment stdout unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("deployment stderr unavailable"))?;
    let (written, status, stdout, stderr, timed_out) =
        std::thread::scope(|scope| -> io::Result<_> {
            let writer = scope.spawn(move || -> io::Result<()> {
                child_stdin.write_all(script.as_bytes())?;
                if let Some(input) = stdin {
                    child_stdin.write_all(input.as_bytes())?;
                }
                Ok(())
            });
            let stdout_reader = scope.spawn(move || bounded_output(&mut stdout));
            let stderr_reader = scope.spawn(move || bounded_output(&mut stderr));
            let deadline = Instant::now() + Duration::from_secs(900);
            let mut timed_out = false;
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    timed_out = true;
                    // Both runners create their own process group. Kill only this
                    // operation, allowing cleanup traps a bounded grace period.
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGTERM);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    break child.wait()?;
                }
                std::thread::sleep(Duration::from_millis(25));
            };
            let written = writer
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("deployment stdin writer panicked")));
            let stdout = stdout_reader
                .join()
                .map_err(|_| io::Error::other("deployment stdout reader panicked"))??;
            let stderr = stderr_reader
                .join()
                .map_err(|_| io::Error::other("deployment stderr reader panicked"))??;
            Ok((written, status, stdout, stderr, timed_out))
        })?;
    let remote = RemoteOutput {
        status_success: status.success() && !timed_out,
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: if timed_out {
            "deployment operation exceeded its 15-minute deadline; inspect the owned target and retained recovery snapshot before retrying".to_string()
        } else {
            String::from_utf8_lossy(&stderr).to_string()
        },
    };
    match written {
        _ if !remote.status_success => Ok(remote),
        Ok(()) => Ok(remote),
        Err(error) => {
            let stderr = last_line(&remote.stderr)
                .map(|line| format!(" (stderr: {line})"))
                .unwrap_or_default();
            Err(io::Error::new(
                error.kind(),
                format!(
                    "ssh {host}: remote exited 0 before consuming all of its piped input: {error}{stderr}"
                ),
            ))
        }
    }
}

fn bounded_output(reader: &mut impl std::io::Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(output);
        }
        // Continue draining after reaching the cap so a verbose subprocess
        // cannot deadlock or exhaust the deployment client's memory.
        let remaining = 2_097_152usize.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
}

pub(super) struct RemoteIdentityPhase {
    pub phase: SetupPhase,
    pub values: Option<RemoteIdentity>,
}

pub(super) struct RemoteIdentity {
    pub home: String,
    pub uid: String,
    pub gid: String,
}

pub(super) fn remote_identity_phase(
    runner: &mut dyn RemoteRunner,
    host: &str,
) -> io::Result<RemoteIdentityPhase> {
    let timer = PhaseTimer::start("remote-identity");
    let output = match runner.run(host, "printf '%s\\n' \"$HOME\" && id -u && id -g", None) {
        Ok(output) => output,
        Err(err) => {
            return Ok(RemoteIdentityPhase {
                phase: timer.finish(SetupStatus::Error, format!("ssh failed: {err}")),
                values: None,
            });
        }
    };
    if output.status_success {
        let mut lines = output.stdout.lines();
        let home = lines.next().unwrap_or("$HOME").trim().to_string();
        let uid = lines.next().unwrap_or("1000").trim().to_string();
        let gid = lines.next().unwrap_or("1000").trim().to_string();
        return Ok(RemoteIdentityPhase {
            phase: timer.finish(SetupStatus::Ok, format!("home={home} uid={uid} gid={gid}")),
            values: Some(RemoteIdentity { home, uid, gid }),
        });
    }
    Ok(RemoteIdentityPhase {
        phase: timer.finish(SetupStatus::Error, output_detail(&output)),
        values: None,
    })
}

pub(super) fn remote_phase(
    runner: &mut dyn RemoteRunner,
    host: &str,
    name: &'static str,
    script: &str,
    stdin: Option<&str>,
) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start(name);
    match runner.run(host, script, stdin) {
        Ok(output) if output.status_success => {
            Ok(timer.finish(SetupStatus::Ok, output_detail(&output)))
        }
        Ok(output) => Ok(timer.finish(SetupStatus::Error, output_detail(&output))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Ok(timer.finish(SetupStatus::Error, "ssh not found on PATH"))
        }
        Err(err) => Ok(timer.finish(SetupStatus::Error, err.to_string())),
    }
}

pub(super) fn skip_phase(name: &'static str, detail: &'static str) -> SetupPhase {
    PhaseTimer::start(name).finish(SetupStatus::Skipped, detail)
}

pub(super) fn append_skipped(
    phases: &mut Vec<SetupPhase>,
    names: &[&'static str],
    detail: &'static str,
) {
    phases.extend(names.iter().map(|name| skip_phase(name, detail)));
}

pub(super) fn phases_have_errors(phases: &[SetupPhase]) -> bool {
    phases
        .iter()
        .any(|phase| matches!(phase.status, SetupStatus::Error))
}

pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// One-line phase detail. A failure leads with the exit status (ssh exits 255
/// for its own connection/auth failures) followed by the last line the remote
/// printed, preferring stderr, so the operator sees the cause rather than "ok".
fn output_detail(output: &RemoteOutput) -> String {
    if output.status_success {
        return last_line(&output.stdout).unwrap_or("ok").to_string();
    }
    let status = match output.exit_code {
        Some(code) => format!("exit status {code}"),
        None => "terminated by signal".to_string(),
    };
    match last_line(&output.stderr).or_else(|| last_line(&output.stdout)) {
        Some(line) => format!("{status}: {line}"),
        None => status,
    }
}

/// The last non-blank line of `text`, trimmed.
fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}

#[cfg(test)]
#[path = "remote_support_tests.rs"]
mod tests;
