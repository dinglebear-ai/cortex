//! Platform-specific installation and transactional container replacement.
use super::*;

/// Upload an explicitly selected artifact, then validate it on the target before
/// replacement. Never substitute a released binary for a caller's custom build.
pub(super) fn stage_binary_override(
    host: &str,
    local: &Path,
    platform: platform::Platform,
) -> io::Result<()> {
    let windows = platform == platform::Platform::WindowsX64;
    let (prepare, remote_path, activate) = if windows {
        (
            "powershell -NoProfile -NonInteractive -Command \"$ErrorActionPreference='Stop'; New-Item -ItemType Directory -Force (Join-Path $HOME '.local/bin') | Out-Null\"",
            ".local/bin/cortex.new.exe",
            "powershell -NoProfile -NonInteractive -Command \"$ErrorActionPreference='Stop'; $bin=Join-Path $HOME '.local/bin/cortex.exe'; $new=Join-Path $HOME '.local/bin/cortex.new.exe'; & $new --version; if ($LASTEXITCODE -ne 0) {throw 'Selected binary cannot run on target'}; if (Test-Path $bin) {Copy-Item $bin ($bin+'.previous') -Force}; Move-Item $new $bin -Force\"",
        )
    } else {
        (
            "set -eu; mkdir -p ~/.local/bin",
            ".local/bin/cortex.new",
            "set -eu; chmod +x ~/.local/bin/cortex.new; ~/.local/bin/cortex.new --version; if test -f ~/.local/bin/cortex; then cp -p ~/.local/bin/cortex ~/.local/bin/cortex.previous; fi; mv -f ~/.local/bin/cortex.new ~/.local/bin/cortex",
        )
    };
    ssh_run(host, prepare)?;
    let status = crate::env::command("scp")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "--",
        ])
        .arg(local)
        .arg(format!("{host}:{remote_path}"))
        .status()?;
    if !status.success() {
        return Err(io::Error::other(
            "explicit agent binary upload failed; installed binary unchanged",
        ));
    }
    ssh_run(host, activate)
}

pub(super) fn run_deploy_windows(
    host: &str,
    local_binary: &Path,
    config: &AgentDeployConfig,
) -> io::Result<()> {
    let persisted = ssh_capture(
        host,
        "powershell -NoProfile -NonInteractive -Command \"$p=Join-Path $HOME '.cortex/heartbeat-agent.env'; if (Test-Path $p) {Get-Content $p}\"",
    )?;
    let pairs = resolve_linux_agent_env(&parse_env_file(&persisted)?, config);
    require_token_if_requested(&pairs, config)?;
    let env_body = render_env_file(&pairs)?;
    ssh_run(
        host,
        platform::service_preflight(platform::Platform::WindowsX64),
    )?;
    if config.binary_override {
        stage_binary_override(host, local_binary, platform::Platform::WindowsX64)?;
    } else {
        ssh_run(host, &platform::windows_stage())?;
    }
    ssh_run_with_stdin(
        host,
        "powershell -NoProfile -NonInteractive -Command \"$ErrorActionPreference='Stop'; $d=Join-Path $HOME '.cortex'; New-Item -ItemType Directory -Force $d | Out-Null; $p=Join-Path $d 'heartbeat-agent.env'; [IO.File]::WriteAllText(($p+'.new'), [Console]::In.ReadToEnd()); Move-Item ($p+'.new') $p -Force\"",
        env_body.as_bytes(),
    )?;
    ssh_run(
        host,
        "powershell -NoProfile -NonInteractive -Command \"$ErrorActionPreference='Stop'; $env:HOME=$HOME; $env:CORTEX_SETUP_PRESERVE_HEARTBEAT_ENV='1'; & (Join-Path $HOME '.local/bin/cortex.exe') setup heartbeatagent install; if ($LASTEXITCODE -ne 0) {exit $LASTEXITCODE}; & (Join-Path $HOME '.local/bin/cortex.exe') setup heartbeatagent check; exit $LASTEXITCODE\"",
    )
}

// Unraid: root fs is a RAM disk — nothing in /root survives reboot.
// Put everything in /mnt/user/appdata/cortex (array, persistent), then
// use `docker run --restart unless-stopped` so Docker itself persists the
// container definition across reboots without any file on disk.
pub(super) const UNRAID_APPDATA: &str = "/mnt/user/appdata/cortex";
pub(super) const UNRAID_ENV: &str = "/mnt/user/appdata/cortex/heartbeat-agent.env";
/// Published agent image. The agent runs the same baked binary as the server
/// (no bind-mounted host binary), pinned to this deploying binary's version so
/// server and agents stay in lockstep. Requires the matching tag to be present
/// on the registry before deploy.
pub(super) const CORTEX_IMAGE_REPO: &str = "ghcr.io/dinglebear-ai/cortex";
pub(super) const UNRAID_HOST_ID: &str = "/mnt/user/appdata/cortex/heartbeat-host-id";
pub(super) const UNRAID_CONTAINER: &str = "cortex-heartbeat-agent";
pub(super) const UNRAID_HOST_SYSLOG: &str = "/var/log/syslog";
pub(super) const UNRAID_CONTAINER_SYSLOG: &str = "/host/var/log/syslog";

pub(super) fn run_deploy_unraid(
    host: &str,
    _local_binary: &Path,
    config: &AgentDeployConfig,
) -> io::Result<()> {
    // Containerized agents run the published image with the binary baked in —
    // no host binary is staged or bind-mounted. Pin to this deploying binary's
    // version so server and agents stay in lockstep (the agent can still self-
    // update between republishes, but the image is the source of truth).
    let image = format!("{CORTEX_IMAGE_REPO}:{}", env!("CARGO_PKG_VERSION"));
    ssh_run(host, &format!("mkdir -p {UNRAID_APPDATA}"))?;

    // Config-preserving upgrade: resolve the container env from the host's
    // persisted heartbeat-agent.env (flag > persisted > default per key, custom
    // keys like CORTEX_AGENT_FILE_TAILS carried through), and carry forward any
    // non-standard mounts (e.g. the host log dir a file-tail reads) from the
    // running container.
    let env_pairs = resolve_agent_env(
        &parse_env_file(&read_optional_remote_file(host, UNRAID_ENV)?)?,
        config,
    );
    require_token_if_requested(&env_pairs, config)?;
    ssh_run(host, &format!("docker pull {image}"))?;

    let custom_mounts: String = preserved_custom_mount_flags(&ssh_capture(
        host,
        &format!(
            "docker inspect {UNRAID_CONTAINER} --format '{MOUNT_INSPECT_TMPL}' 2>/dev/null || true"
        ),
    )?)
    .concat();

    let env_body = render_env_file(&env_pairs)?;
    let staged_env = format!(
        "{UNRAID_ENV}.new-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let docker_body = render_docker_process_env(&env_pairs)?;
    let agent_body = render_env_file(
        &env_pairs
            .iter()
            .filter(|(key, _)| {
                crate::setup::heartbeat_agent_env::RECOGNIZED_KEYS.contains(&key.as_str())
            })
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    for (suffix, body) in [
        ("", env_body),
        (".agent", agent_body),
        (".docker", docker_body),
    ] {
        ssh_run_with_stdin(
            host,
            &format!(
                "umask 077; cat > {staged} && chmod 600 {staged}",
                staged = shell_quote(&format!("{staged_env}{suffix}"))
            ),
            body.as_bytes(),
        )?;
    }
    let run = format!(
        "docker run -d --name {UNRAID_CONTAINER} --restart unless-stopped \\\n         --network host --user 0:0 --no-healthcheck --env-file {UNRAID_ENV}.docker \\\n         -v {UNRAID_APPDATA}:{UNRAID_APPDATA} -v /var/run/docker.sock:/var/run/docker.sock \\\n         -v {UNRAID_HOST_SYSLOG}:{UNRAID_CONTAINER_SYSLOG}:ro {custom_mounts}{image} \\\n         cortex heartbeat agent --env-file {UNRAID_ENV}.agent --host-id-path {UNRAID_HOST_ID}"
    );
    let script = unraid::replacement_script(
        UNRAID_CONTAINER,
        UNRAID_ENV,
        &staged_env,
        &run,
        env!("CARGO_PKG_VERSION"),
    );
    use crate::deploy::remote_support::{RemoteRunner, SshRemoteRunner};
    let output = SshRemoteRunner.run(host, &script, None)?;
    if !output.status_success {
        return Err(io::Error::other(format!(
            "Unraid agent replacement failed on {host}; previous container/config restoration attempted; inspect {UNRAID_ENV}.deploy-lock and retained .rollback-* files if recovery failed (exit {:?})",
            output.exit_code
        )));
    }
    Ok(())
}
