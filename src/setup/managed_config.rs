//! Managed setup uses the runtime declarations and published configuration schema.
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

/// Environment keys declared by the runtime, agent contract, and Compose template.
/// Reading these authored schemas avoids another hand-maintained setup allowlist.
pub fn supported_env_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for schema in [
        include_str!("../config.rs"),
        include_str!("../../.env.example"),
        include_str!("../../docker-compose.prod.yml"),
        include_str!("../heartbeat_agent.rs"),
    ] {
        for word in
            schema.split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        {
            if word.starts_with("CORTEX_") && word.len() > 7 {
                keys.insert(word.to_string());
            }
        }
    }
    for key in [
        "NO_AUTH",
        "RUST_LOG",
        "DOCKER_NETWORK",
        "COMPOSE_PROJECT_NAME",
    ] {
        keys.insert(key.into());
    }
    // These choose execution paths; persisting them into a container changes its identity.
    for key in [
        "CORTEX_HOME",
        "CORTEX_ENV_FILE",
        "CORTEX_SETUP_PRESERVE_HEARTBEAT_ENV",
    ] {
        keys.remove(key);
    }
    keys.insert("CORTEX_SETUP_READY_TIMEOUT_SECS".into());
    keys
}

pub fn secret_key(key: &str) -> bool {
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "API_KEY",
        "APPRISE_URL",
        "WEBHOOK",
        "CONNECTION_STRING",
    ]
    .iter()
    .any(|part| key.contains(part))
}

/// Credentials can also live in an otherwise ordinary endpoint setting.
pub fn sensitive_value(value: &str) -> bool {
    value.split(',').any(|endpoint| {
        url::Url::parse(endpoint.trim()).is_ok_and(|url| {
            !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
        })
    })
}

pub fn redacted_value(key: &str, value: &str) -> String {
    if secret_key(key) || sensitive_value(value) {
        if value.is_empty() {
            "(unset)".into()
        } else {
            "[REDACTED]".into()
        }
    } else {
        value.into()
    }
}

pub fn validate_setting(key: &str, value: &str) -> io::Result<()> {
    if !supported_env_keys().contains(key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown managed configuration key: {key}"),
        ));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{key} must be a single-line value"),
        ));
    }
    Ok(())
}

pub(crate) fn capture_process_settings(values: &mut BTreeMap<String, String>) -> io::Result<()> {
    for key in supported_env_keys() {
        if let Some(value) = crate::config::config_env_var(&key) {
            validate_setting(&key, &value)?;
            values.insert(key, value);
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct EffectiveSetting {
    pub key: String,
    pub value: String,
    pub origin: &'static str,
}

/// A redacted view of the exact environment that repair would persist.
pub fn effective_environment(home: &Path) -> io::Result<Vec<EffectiveSetting>> {
    let path = home.join(".env");
    let persisted = match std::fs::read_to_string(&path) {
        Ok(raw) => super::firstrun::parse_env(&raw),
        Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e),
    };
    let mut values = persisted.clone();
    super::firstrun::populate_env_defaults(&mut values, &home.join("data"))?;
    Ok(values
        .into_iter()
        .map(|(key, value)| {
            let origin = if crate::config::config_env_var(&key).is_some() {
                "process_or_explicit_option"
            } else if persisted.contains_key(&key) {
                "managed_file"
            } else {
                "default"
            };
            let value = redacted_value(&key, &value);
            EffectiveSetting { key, value, origin }
        })
        .collect())
}

#[cfg(test)]
#[path = "managed_config_tests.rs"]
mod tests;
