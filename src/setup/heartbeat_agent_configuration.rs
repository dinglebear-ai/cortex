//! Managed collector environment and collection delivery checks.
use super::*;
use std::time::SystemTime;

pub(super) fn check_capabilities_and_delivery(
    env_path: &Path,
    backend: ServiceBackend,
) -> Vec<SetupPhase> {
    let values = match load_private_agent_env(env_path) {
        Ok(values) => values,
        Err(error) => {
            return vec![PhaseTimer::start("heartbeat-agent-capabilities").finish(
                SetupStatus::Error,
                format!("managed env unreadable: {error}"),
            )];
        }
    };
    let state = |key| {
        if enabled(&values, key) {
            "enabled"
        } else {
            "disabled"
        }
    };
    let journald = if matches!(
        backend,
        ServiceBackend::Launchd | ServiceBackend::WindowsTask
    ) {
        "n/a"
    } else {
        state("CORTEX_AGENT_JOURNALD")
    };
    let ledger = format!(
        "heartbeat=enabled; transcripts={}; docker={}; journald={journald}; shell_history={}; agent_commands={}; file_tails={}; syslog_file={}; auto_update={}",
        state(heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV),
        state("CORTEX_AGENT_DOCKER"),
        state("CORTEX_AGENT_SHELL_HISTORY_FORWARD"),
        state("CORTEX_AGENT_COMMAND_FORWARD"),
        if values
            .get("CORTEX_AGENT_FILE_TAILS")
            .is_some_and(|v| !v.trim().is_empty())
        {
            "enabled"
        } else {
            "disabled"
        },
        if values
            .get("CORTEX_AGENT_SYSLOG_FILE")
            .is_some_and(|v| !v.trim().is_empty())
        {
            "enabled"
        } else {
            "disabled"
        },
        state("CORTEX_AGENT_AUTO_UPDATE"),
    );
    let mut phases =
        vec![PhaseTimer::start("heartbeat-agent-capabilities").finish(SetupStatus::Ok, ledger)];
    match capabilities::discover_from(env_path) {
        Ok(capabilities) => {
            let detail = capabilities
                .iter()
                .map(|c| format!("{}={:?}", c.id, c.status))
                .collect::<Vec<_>>()
                .join("; ");
            let status = if capabilities.iter().any(|c| {
                c.status == CapabilityStatus::NeedsConfiguration && enabled(&values, &c.env_key)
            }) {
                SetupStatus::Warn
            } else {
                SetupStatus::Ok
            };
            phases.push(
                PhaseTimer::start("heartbeat-agent-capability-discovery").finish(status, detail),
            );
        }
        Err(error) => phases.push(
            PhaseTimer::start("heartbeat-agent-capability-discovery")
                .finish(SetupStatus::Warn, format!("discovery unavailable: {error}")),
        ),
    }
    if enabled(&values, heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV) {
        let checkpoint = values
            .get("CORTEX_AGENT_AI_TRANSCRIPT_CHECKPOINT")
            .map(PathBuf::from)
            .or_else(|| {
                crate::setup::cortex_home_dir()
                    .ok()
                    .map(|home| home.join("ai-transcript-forward-checkpoint.json"))
            });
        let (status, detail) = match checkpoint.and_then(|path| std::fs::metadata(path).ok()) {
            Some(metadata) => match metadata
                .modified()
                .ok()
                .and_then(|time| SystemTime::now().duration_since(time).ok())
            {
                Some(age) if age <= Duration::from_secs(120) => (
                    SetupStatus::Ok,
                    format!("checkpoint-fresh; age_secs={}", age.as_secs()),
                ),
                Some(age) => (
                    SetupStatus::Warn,
                    format!("checkpoint-stale; age_secs={}", age.as_secs()),
                ),
                None => (SetupStatus::Warn, "checkpoint-age-unavailable".to_string()),
            },
            None => (SetupStatus::Warn, "checkpoint-missing".to_string()),
        };
        phases
            .push(PhaseTimer::start("heartbeat-agent-transcript-delivery").finish(status, detail));
    }
    phases
}

pub(super) fn write_heartbeat_agent_env(env_path: &Path) -> io::Result<SetupPhase> {
    let timer = PhaseTimer::start("heartbeat-agent-env");
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if crate::env::var(PRESERVE_ENV).as_deref() == Ok("1") && env_path.is_file() {
        return Ok(timer.finish(
            SetupStatus::Ok,
            format!("preserved existing {}", env_path.display()),
        ));
    }
    let existing = if env_path.exists() {
        load_private_agent_env(env_path)?
    } else {
        Default::default()
    };
    let existing_value = |key: &str| existing.get(key).cloned();
    let target = crate::env::var("CORTEX_HEARTBEAT_TARGET")
        .ok()
        .or_else(|| existing_value("CORTEX_HEARTBEAT_TARGET"))
        .or_else(|| read_setup_env_value("CORTEX_HEARTBEAT_TARGET"))
        .unwrap_or_else(|| heartbeat_agent::DEFAULT_TARGET.to_string());
    let token = crate::env::var("CORTEX_HEARTBEAT_TOKEN")
        .ok()
        .or_else(|| existing_value("CORTEX_HEARTBEAT_TOKEN"))
        .or_else(|| read_setup_env_value("CORTEX_TOKEN"))
        .or_else(|| read_setup_env_value("CORTEX_HEARTBEAT_TOKEN"));
    let docker = crate::env::var("CORTEX_AGENT_DOCKER")
        .ok()
        .or_else(|| existing_value("CORTEX_AGENT_DOCKER"))
        .unwrap_or_else(|| "false".to_string());
    let journald = crate::env::var("CORTEX_AGENT_JOURNALD")
        .ok()
        .or_else(|| existing_value("CORTEX_AGENT_JOURNALD"))
        .unwrap_or_else(|| "false".to_string());
    let docker_url = crate::env::var("CORTEX_AGENT_DOCKER_URL")
        .ok()
        .or_else(|| existing_value("CORTEX_AGENT_DOCKER_URL"))
        .unwrap_or_else(|| heartbeat_agent::DEFAULT_DOCKER_URL.to_string());
    let rust_log = crate::env::var("RUST_LOG")
        .ok()
        .or_else(|| existing_value("RUST_LOG"))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "warn".to_string());
    let syslog_file = crate::env::var("CORTEX_AGENT_SYSLOG_FILE")
        .ok()
        .or_else(|| existing_value("CORTEX_AGENT_SYSLOG_FILE"));
    let syslog_target = crate::env::var("CORTEX_SYSLOG_TARGET")
        .ok()
        .or_else(|| existing_value("CORTEX_SYSLOG_TARGET"));
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
    let transcript_forward = crate::env::var(heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV)
        .ok()
        .or_else(|| existing_value(heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV))
        .or_else(|| crate::env::var(heartbeat_agent::AI_TRANSCRIPT_FORWARD_LEGACY_ENV).ok())
        .filter(|value| !value.trim().is_empty());
    if let Some(value) = transcript_forward {
        body.push_str(&format!(
            "{}={}\n",
            heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV,
            shell_safe_value(&value)?
        ));
    }
    for key in heartbeat_agent::OPTIONAL_ENV_KEYS {
        if matches!(
            *key,
            heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV
                | heartbeat_agent::AI_TRANSCRIPT_FORWARD_LEGACY_ENV
        ) {
            continue;
        }
        if let Some(value) = crate::env::var(key)
            .ok()
            .or_else(|| existing_value(key))
            .or_else(|| {
                if *key == "CORTEX_AGENT_AI_TRANSCRIPT_CHECKPOINT" {
                    crate::setup::cortex_home_dir()
                        .ok()
                        .map(|home| home.join("agent-ai-transcript-checkpoint.json"))
                        .filter(|path| path.is_file())
                        .map(|path| path.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .filter(|v| !v.trim().is_empty())
        {
            body.push_str(&format!("{key}={}\n", shell_safe_value(&value)?));
        }
    }
    write_private_file(env_path, &body)?;
    Ok(timer.finish(SetupStatus::Ok, format!("wrote {}", env_path.display())))
}
