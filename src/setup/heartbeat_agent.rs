use std::io::{self, ErrorKind};
use std::path::Path;
use std::time::Instant;

use tracing::warn;

use crate::heartbeat_agent;

use super::firstrun::parse_env;
use super::systemd::{systemctl_user_named_phase, systemctl_user_state};
use super::{
    HeartbeatAgentAction, PhaseTimer, SetupPhase, SetupReport, SetupStatus, check_file_phase,
    host_local_report_input, setup_path_value, setup_report, write_private_file,
};

const UNIT_NAME: &str = "cortex-heartbeat-agent.service";

pub async fn run_heartbeat_agent_setup(action: HeartbeatAgentAction) -> io::Result<SetupReport> {
    let started = Instant::now();
    let home = super::cortex_home_dir()?;
    let env_path = home.join("heartbeat-agent.env");
    let compose_dir = home.join("compose");
    let data_dir = home.join("data");
    let user_home = super::user_home_dir()?;
    let unit_dir = user_home.join(".config/systemd/user");
    let unit_path = unit_dir.join(UNIT_NAME);
    let host_id_path = home.join("heartbeat-host-id");
    let mut phases = Vec::new();

    match action {
        HeartbeatAgentAction::Install => {
            let cortex_bin = super::resolve_cortex_binary()?;
            phases.push(write_heartbeat_agent_env(&env_path)?);
            if has_systemd() {
                std::fs::create_dir_all(&unit_dir)?;
                phases.push(write_heartbeat_agent_unit(
                    &unit_path,
                    &cortex_bin,
                    &env_path,
                    &host_id_path,
                )?);
                phases.push(systemctl_user_named_phase(
                    "heartbeat-agent-daemon-reload",
                    &["daemon-reload"],
                ));
                phases.push(systemctl_user_named_phase(
                    "heartbeat-agent-enabled",
                    &["enable", "--now", UNIT_NAME],
                ));
            } else {
                // No systemd (e.g. Unraid) — install as a Docker Compose service.
                let compose_dir = home.join("compose");
                std::fs::create_dir_all(&compose_dir)?;
                phases.push(write_heartbeat_agent_compose(
                    &compose_dir,
                    &cortex_bin,
                    &env_path,
                    &host_id_path,
                )?);
                phases.push(docker_compose_up_phase(&compose_dir));
            }
        }
        HeartbeatAgentAction::Remove => {
            phases.push(systemctl_user_named_phase(
                "heartbeat-agent-disabled",
                &["disable", "--now", UNIT_NAME],
            ));
            phases.push(remove_file_phase("heartbeat-agent-unit", &unit_path)?);
            phases.push(systemctl_user_named_phase(
                "heartbeat-agent-daemon-reload",
                &["daemon-reload"],
            ));
        }
        HeartbeatAgentAction::Check => {
            let cortex_bin = super::resolve_cortex_binary()?;
            phases.push(check_file_phase(
                "heartbeat-agent-env",
                &env_path,
                "run cortex setup heartbeatagent install",
            ));
            phases.push(check_file_phase(
                "heartbeat-agent-unit",
                &unit_path,
                "run cortex setup heartbeatagent install",
            ));
            phases.push(check_heartbeat_agent_content(
                &unit_path,
                &cortex_bin,
                &env_path,
                &host_id_path,
            ));
            phases.push(heartbeat_agent_enabled_phase());
            phases.push(heartbeat_agent_active_phase());
        }
    }

    Ok(setup_report(
        host_local_report_input(
            action.as_str(),
            started.elapsed().as_millis(),
            home,
            env_path,
            compose_dir,
            data_dir,
        ),
        phases,
    ))
}

fn write_heartbeat_agent_env(env_path: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("heartbeat-agent-env");
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let target = std::env::var("CORTEX_HEARTBEAT_TARGET")
        .ok()
        .or_else(|| read_setup_env_value("CORTEX_HEARTBEAT_TARGET"))
        .unwrap_or_else(|| heartbeat_agent::DEFAULT_TARGET.to_string());
    let token = std::env::var("CORTEX_HEARTBEAT_TOKEN")
        .ok()
        .or_else(|| read_setup_env_value("CORTEX_TOKEN"))
        .or_else(|| read_setup_env_value("CORTEX_HEARTBEAT_TOKEN"));
    let docker = std::env::var("CORTEX_AGENT_DOCKER")
        .ok()
        .unwrap_or_else(|| "false".to_string());
    let journald = std::env::var("CORTEX_AGENT_JOURNALD")
        .ok()
        .unwrap_or_else(|| "false".to_string());
    let docker_url = std::env::var("CORTEX_AGENT_DOCKER_URL")
        .ok()
        .unwrap_or_else(|| heartbeat_agent::DEFAULT_DOCKER_URL.to_string());
    let rust_log = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "warn".to_string());
    let syslog_file = std::env::var("CORTEX_AGENT_SYSLOG_FILE").ok();
    let syslog_target = std::env::var("CORTEX_SYSLOG_TARGET").ok();
    let mut body = format!(
        "CORTEX_HEARTBEAT_TARGET={}\nRUST_LOG={}\nCORTEX_AGENT_DOCKER={}\nCORTEX_AGENT_DOCKER_URL={}\nCORTEX_AGENT_JOURNALD={}\n",
        shell_safe_value(&target)?,
        shell_safe_value(&rust_log)?,
        shell_safe_value(&docker)?,
        shell_safe_value(&docker_url)?,
        shell_safe_value(&journald)?,
    );
    if let Some(syslog_file) = syslog_file.filter(|value| !value.trim().is_empty()) {
        body.push_str(&format!(
            "CORTEX_AGENT_SYSLOG_FILE={}\n",
            shell_safe_value(&syslog_file)?
        ));
    }
    if let Some(syslog_target) = syslog_target.filter(|value| !value.trim().is_empty()) {
        body.push_str(&format!(
            "CORTEX_SYSLOG_TARGET={}\n",
            shell_safe_value(&syslog_target)?
        ));
    }
    if let Some(token) = token.filter(|value| !value.trim().is_empty()) {
        body.push_str(&format!(
            "CORTEX_HEARTBEAT_TOKEN={}\n",
            shell_safe_value(&token)?
        ));
    }
    for key in heartbeat_agent::OPTIONAL_ENV_KEYS {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                body.push_str(&format!("{key}={}\n", shell_safe_value(&value)?));
            }
        }
    }
    write_private_file(env_path, &body)?;
    Ok(timer.finish(SetupStatus::Ok, format!("wrote {}", env_path.display())))
}

fn write_heartbeat_agent_unit(
    unit_path: &Path,
    cortex_bin: &Path,
    env_path: &Path,
    host_id_path: &Path,
) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("heartbeat-agent-unit");
    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        unit_path,
        heartbeat_agent_unit(cortex_bin, env_path, host_id_path)?,
    )?;
    Ok(timer.finish(SetupStatus::Ok, format!("wrote {}", unit_path.display())))
}

fn check_heartbeat_agent_content(
    unit_path: &Path,
    cortex_bin: &Path,
    env_path: &Path,
    host_id_path: &Path,
) -> SetupPhase {
    let timer = PhaseTimer::start("heartbeat-agent-content");
    let expected = match heartbeat_agent_unit(cortex_bin, env_path, host_id_path) {
        Ok(expected) => expected,
        Err(error) => return timer.finish(SetupStatus::Error, error.to_string()),
    };
    match std::fs::read_to_string(unit_path) {
        Ok(current) if current == expected => timer.finish(
            SetupStatus::Ok,
            "heartbeat agent unit matches generated content",
        ),
        Ok(_) => timer.finish(
            SetupStatus::Error,
            format!(
                "{} does not match generated heartbeat agent unit",
                unit_path.display()
            ),
        ),
        Err(error) => timer.finish(SetupStatus::Error, error.to_string()),
    }
}

fn heartbeat_agent_enabled_phase() -> SetupPhase {
    let timer = PhaseTimer::start("heartbeat-agent-enabled");
    match systemctl_user_state("is-enabled", UNIT_NAME).as_deref() {
        Some("enabled") => timer.finish(SetupStatus::Ok, "enabled"),
        Some(state) => timer.finish(SetupStatus::Warn, state),
        None => timer.finish(SetupStatus::Warn, "unknown"),
    }
}

fn heartbeat_agent_active_phase() -> SetupPhase {
    let timer = PhaseTimer::start("heartbeat-agent-active");
    match systemctl_user_state("is-active", UNIT_NAME).as_deref() {
        Some("active") => timer.finish(SetupStatus::Ok, "active"),
        Some(state) => timer.finish(SetupStatus::Warn, state),
        None => timer.finish(SetupStatus::Warn, "unknown"),
    }
}

fn remove_file_phase(name: &'static str, path: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start(name);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(timer.finish(SetupStatus::Ok, format!("removed {}", path.display()))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(timer.finish(
            SetupStatus::Ok,
            format!("{} already absent", path.display()),
        )),
        Err(error) => Err(error),
    }
}

fn heartbeat_agent_unit(
    cortex_bin: &Path,
    env_path: &Path,
    host_id_path: &Path,
) -> io::Result<String> {
    let read_write_dir = setup_path_value(host_id_path.parent().unwrap_or_else(|| Path::new("/")))?;
    let read_write_bin_dir =
        setup_path_value(cortex_bin.parent().unwrap_or_else(|| Path::new("/")))?;
    // The agent-command-forward stream (--agent-command-forward) needs
    // read-write access to the spool's directory too, which normally lives
    // outside `~/.cortex`/`~/.local/bin` (default `~/.local/state/cortex`) —
    // without this, `ProtectHome=read-only` blocks it from ever opening the
    // spool for the truncate-after-forward step.
    let mut read_write_paths_set = vec![read_write_dir.clone(), read_write_bin_dir.clone()];
    if let Ok(spool_path) = super::default_agent_command_spool_path() {
        if let Some(spool_dir) = spool_path.parent() {
            if let Ok(spool_dir) = setup_path_value(spool_dir) {
                if !read_write_paths_set.contains(&spool_dir) {
                    read_write_paths_set.push(spool_dir);
                }
            }
        }
    }
    let read_write_paths = read_write_paths_set.join(" ");
    let cortex_bin = setup_path_value(cortex_bin)?;
    let env_path = setup_path_value(env_path)?;
    let host_id_path = setup_path_value(host_id_path)?;
    Ok(format!(
        "[Unit]\nDescription=cortex heartbeat agent\nDocumentation=https://github.com/dinglebear-ai/cortex\nAfter=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=300\nStartLimitBurst=5\n\n[Service]\nType=simple\nEnvironmentFile={env_path}\nExecStart={cortex_bin} heartbeat agent --host-id-path {host_id_path}\nRestart=on-failure\nRestartSec=5\nUMask=0077\nNoNewPrivileges=true\nPrivateTmp=true\nProtectSystem=strict\nProtectHome=read-only\nReadWritePaths={}\n\n[Install]\nWantedBy=default.target\n",
        read_write_paths
    ))
}

/// Returns true when systemd --user is available on this host.
fn has_systemd() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "--no-pager", "status"])
        .output()
        .map(|o| o.status.code() != Some(127))
        .unwrap_or(false)
}

fn write_heartbeat_agent_compose(
    compose_dir: &Path,
    cortex_bin: &Path,
    env_path: &Path,
    host_id_path: &Path,
) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("heartbeat-agent-compose");
    let compose_path = compose_dir.join("docker-compose.yml");
    let content = heartbeat_agent_compose(cortex_bin, env_path, host_id_path)?;
    std::fs::write(&compose_path, &content)?;
    Ok(timer.finish(SetupStatus::Ok, format!("wrote {}", compose_path.display())))
}

fn docker_compose_up_phase(compose_dir: &Path) -> SetupPhase {
    let timer = PhaseTimer::start("heartbeat-agent-docker-up");
    let result = std::process::Command::new("docker")
        .args(["compose", "up", "-d", "--remove-orphans"])
        .current_dir(compose_dir)
        .output();
    match result {
        Ok(out) if out.status.success() => timer.finish(SetupStatus::Ok, "container started"),
        Ok(out) => timer.finish(
            SetupStatus::Warn,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("docker compose up failed")
                .to_string(),
        ),
        Err(e) => timer.finish(SetupStatus::Warn, e.to_string()),
    }
}

fn heartbeat_agent_compose(
    _cortex_bin: &Path,
    env_path: &Path,
    host_id_path: &Path,
) -> io::Result<String> {
    let data_dir = host_id_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/"));
    // The agent runs the published image with the binary baked in — no host
    // binary is staged or bind-mounted. `user: "0:0"` overrides the image's
    // unprivileged server user (the agent reads root-owned host files), and the
    // image's server health probe is disabled (the agent runs no HTTP server).
    let image = format!("ghcr.io/dinglebear-ai/cortex:{}", env!("CARGO_PKG_VERSION"));
    let env_path = setup_path_value(env_path)?;
    let host_id_path = setup_path_value(host_id_path)?;
    let data_dir = setup_path_value(data_dir)?;
    Ok(format!(
        "services:\n  cortex-heartbeat-agent:\n    image: {image}\n    restart: unless-stopped\n    network_mode: host\n    user: \"0:0\"\n    healthcheck:\n      disable: true\n    env_file: {env_path}\n    volumes:\n      - {data_dir}:{data_dir}\n    command:\n      - cortex\n      - heartbeat\n      - agent\n      - --host-id-path\n      - {host_id_path}\n"
    ))
}

fn read_setup_env_value(key: &str) -> Option<String> {
    let path = super::cortex_home_dir().ok()?.join(".env");
    match std::fs::read_to_string(&path) {
        Ok(raw) => parse_env(&raw).remove(key),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            warn!(path = %path.display(), error = %error, "could not read env file for heartbeat setup");
            None
        }
    }
}

fn shell_safe_value(value: &str) -> io::Result<String> {
    if value
        .chars()
        .any(|ch| ch.is_control() || ch == '\n' || ch == '\r')
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "heartbeat environment value contains unsupported characters",
        ));
    }
    Ok(value.to_string())
}

#[cfg(test)]
#[path = "heartbeat_agent_tests.rs"]
mod tests;
