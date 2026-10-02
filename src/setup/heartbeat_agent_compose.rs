//! Bind only selected host collection sources into the container backend.
use super::enabled;
use crate::setup::heartbeat_agent_env::load_private_agent_env;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

pub(super) fn compose_dir(home: &Path) -> PathBuf {
    let dedicated = home.join("heartbeat-agent-compose");
    if dedicated.join("docker-compose.yml").exists() {
        return dedicated;
    }
    let legacy = home.join("compose");
    if let Ok(raw) = std::fs::read_to_string(legacy.join("docker-compose.yml"))
        && let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&raw)
        && let Some(services) = doc["services"].as_mapping()
        && services.len() == 1
        && services.contains_key("cortex-heartbeat-agent")
    {
        return legacy;
    }
    dedicated
}

pub(super) fn custom_journal_image(compose_dir: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(compose_dir.join("docker-compose.override.yml")) else {
        return false;
    };
    let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&raw) else {
        return false;
    };
    let service = &doc["services"]["cortex-heartbeat-agent"];
    service["image"].as_str().is_some_and(|s| !s.is_empty()) || !service["build"].is_null()
}

pub(super) fn collection_mounts(env_path: &Path) -> io::Result<String> {
    let values = if env_path.is_file() {
        load_private_agent_env(env_path)?
    } else {
        BTreeMap::new()
    };
    let mut paths = BTreeMap::<PathBuf, bool>::new();
    let mut add = |path: PathBuf, readonly: bool| -> io::Result<()> {
        if !path.is_absolute()
            || path == Path::new("/")
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "collection mount must be an absolute non-root path without traversal",
            ));
        }
        paths
            .entry(path)
            .and_modify(|existing| *existing &= readonly)
            .or_insert(readonly);
        Ok(())
    };
    if enabled(&values, "CORTEX_AGENT_DOCKER")
        && let Some(path) = values
            .get("CORTEX_AGENT_DOCKER_URL")
            .and_then(|v| v.strip_prefix("unix://"))
    {
        add(PathBuf::from(path), true)?;
    }
    if enabled(&values, crate::heartbeat_agent::AI_TRANSCRIPT_FORWARD_ENV) {
        for root in crate::scanner::providers::transcript_roots()
            .into_iter()
            .filter(|p| p.is_dir())
        {
            add(root, true)?;
        }
    }
    if enabled(&values, "CORTEX_AGENT_SHELL_HISTORY_FORWARD") {
        let home = crate::setup::user_home_dir()?;
        let zsh = home.join(".zsh_history");
        if zsh.is_file() {
            // Zsh saves history by atomic rename; a file bind would remain on
            // the old inode and silently miss every later history update.
            add(home.clone(), true)?;
        }
        let atuin = home.join(".local/share/atuin");
        if atuin.is_dir() {
            add(atuin, true)?;
        }
    }
    if enabled(&values, "CORTEX_AGENT_COMMAND_FORWARD") {
        let spool = values
            .get("CORTEX_AGENT_COMMAND_SPOOL")
            .map(PathBuf::from)
            .unwrap_or(crate::setup::default_agent_command_spool_path()?);
        if let Some(parent) = spool.parent() {
            add(parent.into(), false)?;
        }
    }
    for source in values
        .get("CORTEX_AGENT_FILE_TAILS")
        .map(|v| crate::agent::syslog_file::parse_file_tails(v))
        .unwrap_or_default()
    {
        if let Some(parent) = source.path.parent() {
            add(
                if parent == Path::new("/") {
                    source.path.clone()
                } else {
                    parent.into()
                },
                true,
            )?;
        }
    }
    if let Some(path) = values
        .get("CORTEX_AGENT_SYSLOG_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        && let Some(parent) = path.parent()
    {
        add(
            if parent == Path::new("/") {
                path.clone()
            } else {
                parent.into()
            },
            true,
        )?;
    }
    if enabled(&values, "CORTEX_AGENT_JOURNALD") {
        let home = env_path
            .parent()
            .ok_or_else(|| io::Error::other("heartbeat env parent missing"))?;
        if !custom_journal_image(&compose_dir(home)) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "journald requires the native systemd backend or an explicit heartbeat-agent Compose image/build override supplying journalctl; no collection settings were disabled",
            ));
        }
        for path in ["/run/log/journal", "/var/log/journal", "/etc/machine-id"] {
            if Path::new(path).exists() {
                add(PathBuf::from(path), true)?;
            }
        }
    }
    let quote = |path: &Path| {
        serde_json::to_string(&path.to_string_lossy().replace('$', "$$")).map_err(io::Error::other)
    };
    let mut mounts = String::new();
    for (path, readonly) in paths {
        mounts.push_str(&format!("      - type: bind\n        source: {}\n        target: {}\n        read_only: {}\n        bind:\n          create_host_path: false\n",quote(&path)?,quote(&path)?,readonly));
    }
    let mut environment = format!(
        "    environment:\n      HOME: {}\n",
        quote(&crate::setup::user_home_dir()?)?
    );
    if let Some(home) = crate::env::var_os("CODEX_HOME") {
        environment.push_str(&format!("      CODEX_HOME: {}\n", quote(Path::new(&home))?));
    }
    Ok(format!("{mounts}{environment}"))
}
