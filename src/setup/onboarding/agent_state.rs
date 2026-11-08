//! A client-only role must not leave an installed collector behind.
use std::{io, path::Path, process::Command, time::Duration};

const INSPECTION_TIMEOUT: Duration = Duration::from_secs(5);

fn native_installation_exists(user_home: &Path) -> bool {
    [
        ".local/lib/cortex/heartbeat-agent/cortex",
        ".local/lib/cortex/heartbeat-agent/cortex.exe",
        ".config/systemd/user/cortex-heartbeat-agent.service",
        "Library/LaunchAgents/ai.dinglebear.cortex-heartbeat-agent.plist",
        "Library/LaunchAgents/ai.dinglebear.cortex-heartbeat.plist",
    ]
    .iter()
    .any(|path| user_home.join(path).exists())
}

fn installed_error() -> io::Error {
    io::Error::other(
        "an installed heartbeat collector belongs to this host; run cortex setup heartbeatagent remove before selecting the client-only role",
    )
}

fn inspection_error(backend: &str) -> io::Error {
    io::Error::other(format!(
        "cannot inspect {backend} heartbeat collectors; restore backend access and run cortex setup heartbeatagent remove before selecting the client-only role"
    ))
}

fn managed_compose_exists(home: &Path) -> io::Result<bool> {
    let dedicated = home.join("heartbeat-agent-compose/docker-compose.yml");
    if dedicated.exists() {
        return Ok(true);
    }
    let legacy = home.join("compose/docker-compose.yml");
    match std::fs::read_to_string(legacy) {
        Ok(raw) => {
            let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&raw) {
                Ok(doc) => doc,
                Err(_) if !raw.contains("cortex-heartbeat-agent") => return Ok(false),
                Err(_) => return Err(inspection_error("managed Compose configuration")),
            };
            Ok(doc["services"]
                .as_mapping()
                .is_some_and(|services| services.contains_key("cortex-heartbeat-agent")))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(inspection_error("managed Compose configuration")),
    }
}

async fn bounded_output(
    mut command: Command,
    timeout: Duration,
) -> io::Result<std::process::Output> {
    command.stdin(std::process::Stdio::null());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "collector inspection timed out"))?
}

fn matches_managed_directory(home: &Path, label: &str) -> bool {
    let actual = Path::new(label.trim());
    [home.join("heartbeat-agent-compose"), home.join("compose")]
        .iter()
        .any(|expected| actual == expected)
}

async fn check_docker(
    home: &Path,
    configured: bool,
    command: Command,
    timeout: Duration,
) -> io::Result<()> {
    match bounded_output(command, timeout).await {
        Ok(output) if output.status.success() => {
            let labels =
                std::str::from_utf8(&output.stdout).map_err(|_| inspection_error("Docker"))?;
            if labels
                .lines()
                .any(|label| matches_managed_directory(home, label))
            {
                return Err(installed_error());
            }
            Ok(())
        }
        _ if configured => Err(inspection_error("Docker")),
        // A missing CLI or unavailable unrelated daemon does not establish an
        // installation when this managed home has no collector configuration.
        _ => Ok(()),
    }
}

#[cfg(any(windows, test))]
fn windows_task_command() -> Command {
    let mut command = crate::env::command("powershell");
    command.args([
        "-NoProfile", "-NonInteractive", "-Command",
        r#"$ErrorActionPreference='Stop'; $identity=[System.Security.Principal.WindowsIdentity]::GetCurrent(); $owned=$false; Get-ScheduledTask -ErrorAction Stop | Where-Object { $_.TaskName -eq 'CortexHeartbeatAgent' -and $_.TaskPath -eq '\' } | ForEach-Object { $owner=$_.Principal.UserId; if ($owner -eq $identity.Name -or $owner -eq $identity.User.Value) { $owned=$true } elseif ($owner -notmatch '^S-1-') { $sid=([System.Security.Principal.NTAccount]::new($owner)).Translate([System.Security.Principal.SecurityIdentifier]).Value; if ($sid -eq $identity.User.Value) { $owned=$true } } }; @{owned=$owned} | ConvertTo-Json -Compress"#,
    ]);
    command
}

#[cfg(any(windows, test))]
async fn check_windows_task(command: Command, timeout: Duration) -> io::Result<()> {
    let output = bounded_output(command, timeout)
        .await
        .map_err(|_| inspection_error("Windows Task Scheduler"))?;
    if !output.status.success() {
        return Err(inspection_error("Windows Task Scheduler"));
    }
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| inspection_error("Windows Task Scheduler"))?;
    match reply["owned"].as_bool() {
        Some(true) => Err(installed_error()),
        Some(false) => Ok(()),
        None => Err(inspection_error("Windows Task Scheduler")),
    }
}

fn docker_inspection_command() -> Command {
    let mut docker = crate::env::command("docker");
    docker.args([
        "ps",
        "--all",
        "--filter",
        "label=com.docker.compose.service=cortex-heartbeat-agent",
        "--format",
        "{{.Label \"com.docker.compose.project.working_dir\"}}",
    ]);
    docker
}

pub(super) async fn ensure_client_only(home: &Path, user_home: &Path) -> io::Result<()> {
    if native_installation_exists(user_home) {
        return Err(installed_error());
    }
    let configured = managed_compose_exists(home)?;
    check_docker(
        home,
        configured,
        docker_inspection_command(),
        INSPECTION_TIMEOUT,
    )
    .await?;
    #[cfg(windows)]
    check_windows_task(windows_task_command(), INSPECTION_TIMEOUT).await?;
    Ok(())
}

#[cfg(test)]
#[path = "agent_state_tests.rs"]
mod tests;
