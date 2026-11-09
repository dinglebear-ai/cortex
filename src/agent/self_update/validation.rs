use super::*;

pub(super) async fn validate_binary(path: &Path, expected_version: &str) -> Result<()> {
    validate_binary_with_deadline(path, expected_version, Duration::from_secs(10)).await
}

async fn validate_binary_with_deadline(
    path: &Path,
    expected_version: &str,
    deadline: Duration,
) -> Result<()> {
    let mut process = crate::env::command(path);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut process, 0);
    let mut command = tokio::process::Command::from(process);
    command
        .arg("--version")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().context("run --version on staged binary")?;
    let (status, stdout, _) = read_version_child(child, deadline).await?;
    if !status.success() {
        bail!("staged binary --version exited with {status}");
    }
    let stdout = String::from_utf8_lossy(&stdout);
    if stdout.split_whitespace().last() != Some(expected_version) {
        bail!(
            "staged binary reported '{}', expected version {expected_version}",
            stdout.trim()
        );
    }
    Ok(())
}

pub(super) fn read_version_child(
    mut child: tokio::process::Child,
    deadline: Duration,
) -> impl std::future::Future<Output = Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)>> {
    // Own group cleanup before the future can be dropped without being polled.
    #[cfg(unix)]
    let mut process_group = ProcessGroupGuard(child.id());
    async move {
        use tokio::io::AsyncReadExt;
        const OUTPUT_LIMIT: u64 = 16 * 1024;
        let stdout = child
            .stdout
            .take()
            .context("capture staged binary stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("capture staged binary stderr")?;
        let bounded_read = |reader: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
            let mut bytes = Vec::new();
            reader
                .take(OUTPUT_LIMIT + 1)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() as u64 > OUTPUT_LIMIT {
                return Err(std::io::Error::other(
                    "staged binary version output exceeded limit",
                ));
            }
            Ok(bytes)
        };
        let result = timeout(deadline, async {
            tokio::try_join!(
                child.wait(),
                bounded_read(Box::new(stdout)),
                bounded_read(Box::new(stderr))
            )
        })
        .await;
        if !matches!(result, Ok(Ok(_))) {
            // Kill and reap explicitly. On cancellation, the Unix group guard
            // terminates descendants and kill_on_drop terminates the direct child.
            #[cfg(unix)]
            process_group.terminate();
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        result
            .with_context(|| format!("staged binary --version exceeded {deadline:?}"))?
            .context("read staged binary version")
    }
}

// Cancellation drops this guard even while stdout/stderr or child.wait() is pending.
// kill_on_drop alone only terminates the direct child, not its process group.
#[cfg(unix)]
struct ProcessGroupGuard(Option<u32>);

#[cfg(unix)]
impl ProcessGroupGuard {
    fn terminate(&mut self) {
        if let Some(pid) = self.0.take() {
            // SAFETY: the validator was spawned into its own fresh process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}
