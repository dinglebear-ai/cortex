use std::io::{self, Read as _, Write as _};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use crate::setup::{PhaseTimer, SetupPhase, SetupStatus};

#[derive(Debug, Clone)]
pub(crate) struct RemoteOutput {
    pub status_success: bool,
    /// The remote exit code; `None` when the ssh process was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) trait RemoteRunner {
    fn latest_stable_version(&mut self) -> io::Result<String> {
        super::release::latest_stable_version()
    }

    fn run(&mut self, host: &str, script: &str, stdin: Option<&str>) -> io::Result<RemoteOutput>;
}

pub(crate) struct SshRemoteRunner;

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
        // Readiness may allow ten minutes; keep one bounded budget for script
        // execution, stdin delivery and output drain on both local and SSH paths.
        let deadline = Duration::from_secs(900);
        feed_and_reap(host, child, script, stdin, deadline)
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
        feed_and_reap(host, child, script, stdin, Duration::from_secs(900))
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
    deadline: Duration,
) -> io::Result<RemoteOutput> {
    let Some(mut child_stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "failed to open ssh stdin",
        ));
    };
    let stdout = child.stdout.take().expect("piped ssh stdout");
    let stderr = child.stderr.take().expect("piped ssh stderr");
    // Owned threads allow the deadline to cover pipe drain as well as process
    // exit: a descendant can keep a pipe open after ssh itself has exited.
    let script = script.to_owned();
    let input = stdin.map(str::to_owned);
    let writer = std::thread::spawn(move || -> io::Result<()> {
        child_stdin.write_all(script.as_bytes())?;
        if let Some(input) = input {
            child_stdin.write_all(input.as_bytes())?;
        }
        Ok(())
    });
    let drain = |mut reader: Box<dyn io::Read + Send>| {
        let mut bytes = Vec::new();
        reader.by_ref().take(1024 * 1024).read_to_end(&mut bytes)?;
        io::copy(&mut reader, &mut io::sink())?;
        Ok::<_, io::Error>(bytes)
    };
    let out_reader = std::thread::spawn(move || drain(Box::new(stdout)));
    let err_reader = std::thread::spawn(move || drain(Box::new(stderr)));
    let started = Instant::now();
    let mut status = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(result) => status = result,
                Err(error) => {
                    terminate_remote_child(&mut child);
                    return Err(error);
                }
            }
        }
        if status.is_some()
            && writer.is_finished()
            && out_reader.is_finished()
            && err_reader.is_finished()
        {
            break;
        }
        if started.elapsed() >= deadline {
            terminate_remote_child(&mut child);
            // Do not join an unfinished pipe thread: even an escaped descendant
            // must not extend the caller's deadline. Its owned handles will close
            // when it eventually exits; normal ssh descendants die with the group.
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("ssh {host}: remote phase exceeded {deadline:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let written = writer
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("ssh stdin writer panicked")));
    let stdout = out_reader
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("ssh stdout reader panicked")))?;
    let stderr = err_reader
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("ssh stderr reader panicked")))?;
    let output = std::process::Output {
        status: status.expect("observed child exit"),
        stdout,
        stderr,
    };
    let remote = RemoteOutput {
        status_success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
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

fn terminate_remote_child(child: &mut Child) {
    #[cfg(unix)]
    {
        // ssh owns a fresh process group; kill descendants holding its pipes too.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
