use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};

use super::types::{
    ContainerInfo, DockerInspect, ListenerInfo, MountInfo, PortInfo, SystemdStatus,
};

/// GNU `timeout` exits 127 when the program it was asked to run does not
/// exist. Every inspector call is wrapped in `timeout`, so a missing binary
/// arrives here as an ordinary non-zero exit rather than an exec error —
/// which is why it has to be recognised by exit code.
fn command_not_found(output: &std::process::Output) -> bool {
    output.status.code() == Some(127)
}

/// Build the `docker_unavailable` error for a failed `docker` invocation,
/// distinguishing "the CLI is not installed" from "the CLI ran and failed".
/// The former is the common deployment case — the runtime image ships no
/// docker client — and a generic "docker inspect failed" hides it behind
/// whatever `timeout` happened to print.
pub(crate) fn docker_cli_error(
    action: &str,
    output: &std::process::Output,
) -> DockerUnavailableError {
    if command_not_found(output) {
        return DockerUnavailableError(
            "`docker` CLI not found on PATH: cortex cannot introspect its own container. \
             Provide a docker client in the runtime image and point DOCKER_HOST at a \
             read-only docker-socket-proxy rather than bind-mounting the docker socket"
                .to_string(),
        );
    }
    DockerUnavailableError(format!(
        "{action} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CliDockerInspect;

impl DockerInspect for CliDockerInspect {
    fn inspect_container(&self, name: &str) -> Result<Option<ContainerInfo>> {
        let output = run_inspector_command(
            "docker",
            &["inspect", name, "--format", "{{json .}}"],
            Duration::from_secs(10),
        )
        .map_err(|e| DockerUnavailableError(format!("docker inspect failed: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            if !stderr.contains("no such object") && !stderr.contains("no such container") {
                return Err(docker_cli_error("docker inspect", &output).into());
            }
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        container_info_from_inspect(value).map(Some)
    }

    fn find_candidates(&self, service: &str, container_name: &str) -> Result<Vec<ContainerInfo>> {
        let filter = format!("label=com.docker.compose.service={service}");
        let output = run_inspector_command(
            "docker",
            &["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"],
            Duration::from_secs(10),
        )
        .map_err(|e| DockerUnavailableError(format!("docker ps failed: {e}")))?;
        if !output.status.success() {
            return Err(docker_cli_error("docker ps", &output).into());
        }
        let names = String::from_utf8_lossy(&output.stdout);
        let mut found = Vec::new();
        for name in names.lines().take(10) {
            if (name == container_name || name.contains(service))
                && let Some(info) = self.inspect_container(name)?
            {
                found.push(info);
            }
        }
        Ok(found)
    }

    fn systemd_status(&self, unit: &str) -> Result<Option<SystemdStatus>> {
        let output = run_inspector_command(
            "systemctl",
            &["--user", "is-active", unit],
            Duration::from_secs(3),
        )?;
        systemd_status_from_output(unit, &output)
    }

    fn listeners(&self, ports: &[u16]) -> Result<Vec<ListenerInfo>> {
        let mut listeners = Vec::new();
        for port in ports {
            let port_arg = format!(":{port}");
            let output = run_inspector_command(
                "ss",
                &["-H", "-ltnup", "sport", "=", &port_arg],
                Duration::from_secs(3),
            )?;
            if !output.status.success() {
                return Err(anyhow!(
                    "ss listener check failed for port {port}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            if ss_output_has_listener(&output.stdout) {
                listeners.push(ListenerInfo {
                    port: *port,
                    process: Some(String::from_utf8_lossy(&output.stdout).to_string()),
                    belongs_to_target: false,
                });
            }
        }
        Ok(listeners)
    }

    fn published_port_owner(&self, port: u16) -> Result<Option<String>> {
        let publish_filter = format!("publish={port}");
        let output = run_inspector_command(
            "docker",
            &[
                "ps",
                "--filter",
                &publish_filter,
                "--format",
                "{{.ID}}\t{{.Names}}",
            ],
            Duration::from_secs(10),
        )
        .map_err(|e| DockerUnavailableError(format!("docker ps failed: {e}")))?;
        if !output.status.success() {
            return Err(docker_cli_error("docker ps", &output).into());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let Some(first) = lines.next() else {
            return Ok(None);
        };
        if lines.next().is_some() {
            return Ok(None);
        }
        let mut fields = first.split('\t');
        let id = fields.next().unwrap_or_default().trim();
        let name = fields.next().unwrap_or_default().trim();
        if !id.is_empty() {
            Ok(Some(id.into()))
        } else if !name.is_empty() {
            Ok(Some(name.into()))
        } else {
            Ok(None)
        }
    }
}

pub(crate) fn ss_output_has_listener(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout)
        .lines()
        .any(|line| !line.trim().is_empty() && !line.trim_start().starts_with("Netid "))
}

pub(crate) fn systemd_status_from_output(
    unit: &str,
    output: &std::process::Output,
) -> Result<Option<SystemdStatus>> {
    if output.status.success() {
        return Ok(Some(SystemdStatus {
            unit: unit.into(),
            active: true,
        }));
    }

    let code = output.status.code();
    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_ascii_lowercase();
    if matches!(code, Some(3) | Some(4))
        || matches!(stdout.as_str(), "inactive" | "failed" | "unknown")
    {
        return Ok(Some(SystemdStatus {
            unit: unit.into(),
            active: false,
        }));
    }

    // A host with no systemd at all — every container, cortex's own included
    // — cannot be running a conflicting `cortex.service`. That is an answer to
    // this probe, not a failure of it, so report "no unit" rather than raising
    // an Error diagnostic that would keep `compose_doctor` red forever.
    if command_not_found(output) {
        tracing::debug!(
            unit,
            "systemctl not present; treating systemd ownership check as not applicable"
        );
        return Ok(None);
    }

    Err(anyhow!(
        "systemctl --user is-active {unit} failed (code={code:?}): {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

pub(super) fn run_inspector_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output> {
    let timeout_secs = timeout.as_secs().max(1).to_string();
    let mut timeout_args = vec!["-k", "1s", &timeout_secs, program];
    timeout_args.extend(args);
    let output = crate::env::command("timeout")
        .args(timeout_args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!("'timeout' binary not found; please install GNU coreutils or add timeout to PATH before running compose diagnostics")
            } else {
                anyhow!("failed to run {program} inspector command: {e}")
            }
        })?;
    if program == "systemctl"
        && args.first() == Some(&"--user")
        && !output.status.success()
        && crate::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
        && systemctl_needs_user_bus_fallback(&output)
        && let Some((runtime_dir, bus_address)) = inferred_user_bus_env()
    {
        let mut retry_args = vec!["-k", "1s", &timeout_secs, program];
        retry_args.extend(args);
        return crate::env::command("timeout")
            .env("XDG_RUNTIME_DIR", runtime_dir)
            .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
            .args(retry_args)
            .output()
            .map_err(|e| anyhow!("failed to run {program} inspector command: {e}"));
    }
    Ok(output)
}

fn systemctl_needs_user_bus_fallback(output: &std::process::Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains("DBUS_SESSION_BUS_ADDRESS") || stderr.contains("user scope bus")
}

fn inferred_user_bus_env() -> Option<(PathBuf, String)> {
    let runtime_dir = PathBuf::from(format!("/run/user/{}", current_uid()));
    let bus = runtime_dir.join("bus");
    bus.exists()
        .then(|| (runtime_dir, format!("unix:path={}", bus.display())))
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

pub(crate) fn container_info_from_inspect(value: serde_json::Value) -> Result<ContainerInfo> {
    let labels = value
        .pointer("/Config/Labels")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let name = value
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or("cortex")
        .trim_start_matches('/')
        .to_string();
    let mounts = value
        .get("Mounts")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|m| MountInfo {
                    source: m.get("Source").and_then(|v| v.as_str()).map(PathBuf::from),
                    target: m
                        .get("Destination")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    kind: m
                        .get("Type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    volume_name: m.get("Name").and_then(|v| v.as_str()).map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ContainerInfo {
        id: value
            .get("Id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        name,
        status: value
            .pointer("/State/Status")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        health: value
            .pointer("/State/Health/Status")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        image: value
            .pointer("/Config/Image")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        image_id: value
            .get("Image")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        labels,
        mounts,
        ports: ports_from_inspect(&value),
    })
}

fn ports_from_inspect(value: &serde_json::Value) -> Vec<PortInfo> {
    let Some(map) = value
        .pointer("/NetworkSettings/Ports")
        .and_then(|v| v.as_object())
    else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for (private, bindings) in map {
        let Some((port, protocol)) = private.split_once('/') else {
            continue;
        };
        let Ok(private_port) = port.parse::<u16>() else {
            continue;
        };
        match bindings {
            serde_json::Value::Array(items) if !items.is_empty() => {
                for item in items {
                    ports.push(PortInfo {
                        private_port,
                        public_port: item
                            .get("HostPort")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse::<u16>().ok()),
                        protocol: protocol.to_string(),
                        host_ip: item
                            .get("HostIp")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    });
                }
            }
            _ => ports.push(PortInfo {
                private_port,
                public_port: None,
                protocol: protocol.to_string(),
                host_ip: None,
            }),
        }
    }
    ports
}

#[derive(Debug)]
pub(crate) struct DockerUnavailableError(pub String);

impl std::fmt::Display for DockerUnavailableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "docker unavailable: {}", self.0)
    }
}

impl std::error::Error for DockerUnavailableError {}
