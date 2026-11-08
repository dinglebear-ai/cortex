//! Local collection discovery and explicit consent persistence.
use crate::setup::heartbeat_agent_env::{atomic_private_write, load_private_agent_env};
use serde::Serialize;
use std::{collections::BTreeMap, io, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Enabled,
    Declined,
    Unavailable,
    NeedsConfiguration,
}
#[derive(Debug, Clone, Serialize)]
pub struct AgentCapability {
    pub id: String,
    pub label: String,
    pub status: CapabilityStatus,
    pub available: bool,
    pub detail: String,
    pub env_key: String,
    #[serde(serialize_with = "serialize_detected_value")]
    pub detected_value: Option<String>,
}

fn serialize_detected_value<S: serde::Serializer>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .as_ref()
        .map(|value| crate::setup::managed_config::redacted_value("", value))
        .serialize(serializer)
}

pub fn discover_agent_capabilities() -> io::Result<Vec<AgentCapability>> {
    discover_from(&crate::setup::cortex_home_dir()?.join("heartbeat-agent.env"))
}

pub(super) fn discover_from(env_path: &std::path::Path) -> io::Result<Vec<AgentCapability>> {
    let values = if env_path.exists() {
        load_private_agent_env(env_path)?
    } else {
        BTreeMap::new()
    };
    discover_with_values(values)
}

pub(crate) fn discover_with_overrides(
    overrides: &BTreeMap<String, String>,
) -> io::Result<Vec<AgentCapability>> {
    let path = crate::setup::cortex_home_dir()?.join("heartbeat-agent.env");
    let mut values = if path.exists() {
        load_private_agent_env(&path)?
    } else {
        BTreeMap::new()
    };
    for (key, value) in overrides {
        if crate::setup::heartbeat_agent_env::RECOGNIZED_KEYS.contains(&key.as_str()) {
            values.insert(key.clone(), value.clone());
        }
    }
    discover_with_values(values)
}

fn discover_with_values(mut values: BTreeMap<String, String>) -> io::Result<Vec<AgentCapability>> {
    let home = crate::setup::user_home_dir()?;
    if !values.contains_key(crate::heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV)
        && let Some(value) = values
            .get(crate::heartbeat_agent::AI_TRANSCRIPT_FORWARD_LEGACY_ENV)
            .cloned()
    {
        values.insert(
            crate::heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV.into(),
            value,
        );
    }
    // An explicit daemon endpoint is a collection target, not a hint. Never
    // fall back to another local daemon when that chosen endpoint is unavailable.
    let socket = values
        .get("CORTEX_AGENT_DOCKER_URL")
        .cloned()
        .or_else(|| crate::env::var("DOCKER_HOST").ok())
        .or_else(|| {
            [
                PathBuf::from("/var/run/docker.sock"),
                home.join(".orbstack/run/docker.sock"),
                home.join(".docker/run/docker.sock"),
            ]
            .into_iter()
            .find(|p| p.exists())
            .map(|p| format!("unix://{}", p.display()))
        });
    let docker_available = socket.as_ref().is_some_and(|v| {
        if let Some(path) = v.strip_prefix("unix://") {
            return std::fs::metadata(path).is_ok();
        }
        reqwest::Url::parse(v).is_ok_and(|url| url.scheme() == "http" && url.host_str().is_some())
            || (cfg!(windows) && v.starts_with("npipe://"))
    });
    let roots = crate::scanner::providers::transcript_roots()
        .into_iter()
        .filter(|p| std::fs::read_dir(p).is_ok())
        .collect::<Vec<_>>();
    // Match the implemented history collectors rather than promising Bash or
    // HISTFILE support that the forwarder does not currently provide.
    let history = [
        home.join(".zsh_history"),
        home.join(".local/share/atuin/history.db"),
    ]
    .into_iter()
    .find(|p| std::fs::File::open(p).is_ok());
    let spool = values
        .get("CORTEX_AGENT_COMMAND_SPOOL")
        .map(PathBuf::from)
        .unwrap_or(crate::setup::default_agent_command_spool_path()?);
    let tails = values
        .get("CORTEX_AGENT_FILE_TAILS")
        .filter(|v| !v.trim().is_empty())
        .cloned();
    let tail_readable = tails.as_ref().is_some_and(|v| {
        v.split(',').all(|spec| {
            spec.rsplit_once(':').is_some_and(|(p, label)| {
                !label.trim().is_empty()
                    && std::path::Path::new(p).is_file()
                    && std::fs::File::open(p).is_ok()
            })
        })
    });
    let syslog_file = values
        .get("CORTEX_AGENT_SYSLOG_FILE")
        .filter(|v| !v.trim().is_empty())
        .cloned();
    let specs = [
        (
            "docker",
            "Docker logs",
            "CORTEX_AGENT_DOCKER",
            docker_available,
            socket.clone(),
            "Requires a local Docker socket or explicitly configured Docker endpoint".to_string(),
        ),
        (
            "transcripts",
            "AI transcripts",
            crate::heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV,
            !roots.is_empty(),
            None,
            format!("{} provider discovery roots present", roots.len()),
        ),
        (
            "journald",
            "Journal logs",
            "CORTEX_AGENT_JOURNALD",
            cfg!(target_os = "linux") && std::path::Path::new("/run/systemd/journal").is_dir(),
            None,
            "Requires a readable systemd journal".to_string(),
        ),
        (
            "shell_history",
            "Shell history",
            "CORTEX_AGENT_SHELL_HISTORY_FORWARD",
            history.is_some(),
            None,
            "Requires Zsh or Atuin history; may contain sensitive commands".to_string(),
        ),
        (
            "agent_commands",
            "Agent commands",
            "CORTEX_AGENT_COMMAND_FORWARD",
            spool.is_file(),
            Some(spool.display().to_string()),
            "Requires the agent-command adapter spool".to_string(),
        ),
        (
            "syslog_file",
            "Configured syslog file",
            "CORTEX_AGENT_SYSLOG_FILE",
            syslog_file.as_ref().is_some_and(|v| {
                std::path::Path::new(v).is_file() && std::fs::File::open(v).is_ok()
            }),
            syslog_file,
            "Configure a readable syslog file path".to_string(),
        ),
        (
            "file_tails",
            "Configured file tails",
            "CORTEX_AGENT_FILE_TAILS",
            tail_readable,
            tails,
            "Configure readable path:label file tails".to_string(),
        ),
    ];
    Ok(specs
        .into_iter()
        .map(|(id, label, key, available, detected_value, detail)| {
            let configured = values.get(key);
            let selected = configured.is_some_and(|v| {
                if matches!(id, "file_tails" | "syslog_file") {
                    !v.is_empty()
                } else {
                    v == "1" || v.eq_ignore_ascii_case("true")
                }
            });
            let status = if selected && available {
                CapabilityStatus::Enabled
            } else if selected {
                CapabilityStatus::NeedsConfiguration
            } else if configured.is_some() {
                CapabilityStatus::Declined
            } else if !available {
                CapabilityStatus::Unavailable
            } else {
                CapabilityStatus::NeedsConfiguration
            };
            AgentCapability {
                id: id.into(),
                label: label.into(),
                status,
                available,
                detail,
                env_key: key.into(),
                detected_value,
            }
        })
        .collect())
}

pub fn configure_agent_capabilities(selected: &[String]) -> io::Result<()> {
    let capabilities = discover_agent_capabilities()?;
    for id in selected {
        let capability = capabilities.iter().find(|c| &c.id == id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown capability {id}"),
            )
        })?;
        if !capability.available {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} needs configuration: {}",
                    capability.label, capability.detail
                ),
            ));
        }
    }
    let path = crate::setup::cortex_home_dir()?.join("heartbeat-agent.env");
    let mut values = if path.exists() {
        load_private_agent_env(&path)?
    } else {
        BTreeMap::new()
    };
    for capability in capabilities {
        let enabled = selected.contains(&capability.id);
        let value = if matches!(capability.id.as_str(), "file_tails" | "syslog_file") {
            if enabled {
                capability.detected_value.clone().unwrap_or_default()
            } else {
                String::new()
            }
        } else {
            enabled.to_string()
        };
        values.insert(capability.env_key, value);
        if enabled
            && capability.id == "docker"
            && let Some(value) = capability.detected_value.clone()
        {
            values.insert("CORTEX_AGENT_DOCKER_URL".into(), value);
        }
        if enabled
            && capability.id == "agent_commands"
            && let Some(value) = capability.detected_value.clone()
        {
            values.insert("CORTEX_AGENT_COMMAND_SPOOL".into(), value);
        }
    }
    let mut body = String::new();
    for (key, value) in values {
        body.push_str(&format!("{key}={}\n", super::shell_safe_value(&value)?));
    }
    atomic_private_write(&path, body.as_bytes(), 0o600)
}
