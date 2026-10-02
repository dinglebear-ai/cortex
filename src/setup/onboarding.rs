//! Role-aware first run, explicit collection consent, and repeatable client setup.
use super::{PhaseTimer, SetupPhase, SetupStatus};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    time::Duration,
};

mod agent_state;
mod input;
pub use input::{Role, StartOptions, read_secret};
use input::{SavedSetup, csv, load_env, prompt};
#[derive(Serialize)]
pub struct StartReport {
    pub role: Role,
    pub server: String,
    pub dry_run: bool,
    pub effective: Vec<super::managed_config::EffectiveSetting>,
    pub capabilities: Vec<super::AgentCapability>,
    pub client_paths: Vec<PathBuf>,
    pub phases: Vec<SetupPhase>,
    pub has_errors: bool,
}

pub async fn run_start(mut options: StartOptions) -> io::Result<StartReport> {
    let home = super::cortex_home_dir()?;
    let saved_path = home.join("setup.toml");
    let saved: SavedSetup = match std::fs::read_to_string(&saved_path) {
        Ok(raw) => toml::from_str(&raw).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid saved setup TOML; inspect setup.toml",
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => SavedSetup::default(),
        Err(e) => return Err(e),
    };
    let interactive = options.interactive && io::stdin().is_terminal() && !options.dry_run;
    let role = match options.role.or(saved.role) {
        Some(role) => role,
        None if interactive => {
            Role::parse(&prompt("Machine role (server/client/agent)", "server")?)?
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "first unattended setup requires --role server|client|agent",
            ));
        }
    };
    let mut values = load_env(&home.join(".env"))?;
    if role == Role::Agent {
        input::restore_agent_enrollment(&home, &mut values)?;
    }
    let persisted_keys: std::collections::BTreeSet<_> = values.keys().cloned().collect();
    let mut explicit_keys: std::collections::BTreeSet<_> =
        super::managed_config::supported_env_keys()
            .into_iter()
            .filter(|key| crate::config::config_env_var(key).is_some())
            .collect();
    explicit_keys.extend(options.settings.keys().cloned());
    if options.token_file.is_some() {
        explicit_keys.insert(
            if role == Role::Agent {
                "CORTEX_HEARTBEAT_TOKEN"
            } else {
                "CORTEX_TOKEN"
            }
            .into(),
        );
    }
    if options.api_token_file.is_some() {
        explicit_keys.insert("CORTEX_API_TOKEN".into());
    }
    super::managed_config::capture_process_settings(&mut values)?;
    for (key, value) in &options.settings {
        super::managed_config::validate_setting(key, value)?;
        values.insert(key.clone(), value.clone());
    }
    if let Some(path) = &options.token_file {
        values.insert(
            if role == Role::Agent {
                "CORTEX_HEARTBEAT_TOKEN"
            } else {
                "CORTEX_TOKEN"
            }
            .into(),
            read_secret(path)?,
        );
    }
    if let Some(path) = &options.api_token_file {
        values.insert("CORTEX_API_TOKEN".into(), read_secret(path)?);
    }
    let default_server = saved
        .server
        .as_deref()
        .or(values.get("CORTEX_SERVER_URL").map(String::as_str))
        .or_else(|| {
            (role == Role::Agent)
                .then(|| values.get("CORTEX_HEARTBEAT_TARGET").map(String::as_str))
                .flatten()
        })
        .unwrap_or("http://127.0.0.1:3100");
    let server = if let Some(url) = options.server.take() {
        url
    } else if interactive && role != Role::Server {
        prompt("Cortex server URL", default_server)?
    } else if role == Role::Server {
        format!(
            "http://127.0.0.1:{}",
            values
                .get("CORTEX_PORT")
                .map(String::as_str)
                .unwrap_or("3100")
        )
    } else {
        default_server.into()
    };
    let server = super::verification::normalize_server_url(&server).map_err(io::Error::other)?;
    input::confirm_credential_transport(interactive, role, &server, &mut values)?;
    if interactive && role == Role::Server && saved.role.is_none() {
        input::prompt_server_settings(&mut values)?;
    }

    if role != Role::Server {
        values.insert("CORTEX_USE_HTTP".into(), "true".into());
        values.insert("CORTEX_SERVER_URL".into(), server.clone());
    }
    if role == Role::Agent {
        values.insert("CORTEX_HEARTBEAT_TARGET".into(), server.clone());
        if values
            .get("CORTEX_HEARTBEAT_TOKEN")
            .is_none_or(|v| v.is_empty())
        {
            if interactive {
                values.insert(
                    "CORTEX_HEARTBEAT_TOKEN".into(),
                    read_secret(Path::new(&prompt("Private ingest token file", "")?))?,
                );
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "agent enrollment requires --token-file or a saved heartbeat token",
                ));
            }
        }
    }
    let oauth = values.get("CORTEX_AUTH_MODE").is_some_and(|v| v == "oauth");
    super::auth_policy::no_auth(&values)?;
    let backup_schedule = options.backup_schedule.unwrap_or(saved.backup_schedule);
    let backup_schedule = if interactive
        && role == Role::Server
        && saved.role.is_none()
        && options.backup_schedule.is_none()
    {
        matches!(
            prompt(
                "Schedule a verified recovery snapshot every six hours? (yes/no)",
                "yes"
            )?
            .as_str(),
            "yes" | "y"
        )
    } else {
        backup_schedule
    };
    if role != Role::Server && backup_schedule {
        return Err(io::Error::other(
            "backup scheduling requires the server role",
        ));
    }
    let clients = match options.clients {
        Some(v) => v,
        None if interactive => csv(&prompt(
            "MCP clients to configure (codex,claude,gemini or none)",
            &saved.clients.join(","),
        )?),
        None => saved.clients,
    };
    input::complete_client_credentials(
        role,
        &clients,
        interactive,
        options.dry_run,
        oauth,
        &mut values,
    )?;
    let user_home = super::user_home_dir()?;
    let mcp_token = if oauth
        && values
            .get("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
            .is_none_or(|v| v != "false")
    {
        None
    } else {
        values.get("CORTEX_TOKEN").map(String::as_str)
    };
    // Validate all existing client files before mutating setup.
    let client_paths =
        super::client_config::configure_clients(&user_home, &clients, &server, mcp_token, true)?;
    let mut detected = super::heartbeat_agent::discover_with_overrides(&values)?;
    let default_capabilities = if saved.role.is_none()
        && role != Role::Client
        && home.join("heartbeat-agent.env").exists()
    {
        let existing =
            super::heartbeat_agent_env::load_private_agent_env(&home.join("heartbeat-agent.env"))?;
        detected
            .iter()
            .filter(|capability| {
                let value = existing.get(&capability.env_key).or_else(|| {
                    (capability.id == "transcripts")
                        .then(|| existing.get("CORTEX_AGENT_AI_TRANSCRIPTS"))
                        .flatten()
                });
                value.is_some_and(|value| {
                    if matches!(capability.id.as_str(), "file_tails" | "syslog_file") {
                        !value.trim().is_empty()
                    } else {
                        value.eq_ignore_ascii_case("true") || value == "1"
                    }
                })
            })
            .map(|capability| capability.id.clone())
            .collect()
    } else {
        saved.capabilities.clone()
    };
    let selected = match options.capabilities {
        Some(v) => v,
        None if interactive && role != Role::Client => {
            for capability in &detected {
                eprintln!(
                    "{}: {:?} — {}",
                    capability.id, capability.status, capability.detail
                );
            }
            csv(&prompt(
                "Collection capabilities to enable (comma-separated; none disables optional collection)",
                &default_capabilities.join(","),
            )?)
        }
        None => default_capabilities,
    };
    if role == Role::Client {
        agent_state::ensure_client_only(&home, &user_home).await?;
    }
    if role == Role::Client && !selected.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "client-only role cannot enable collection; select agent or server",
        ));
    }
    for id in &selected {
        let c = detected
            .iter()
            .find(|c| &c.id == id)
            .ok_or_else(|| io::Error::other(format!("unknown capability {id}")))?;
        // Agent-command adapter will be installed by this flow.
        if !c.available && id != "agent_commands" {
            return Err(io::Error::other(format!(
                "{} needs configuration: {}",
                c.label, c.detail
            )));
        }
    }
    for capability in &mut detected {
        if selected.contains(&capability.id) {
            capability.status = if capability.available {
                super::CapabilityStatus::Enabled
            } else {
                super::CapabilityStatus::NeedsConfiguration
            };
        } else if capability.available {
            capability.status = super::CapabilityStatus::Declined;
        }
    }
    let explicit: HashMap<_, _> = values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let _overlay = crate::config::PluginEnvGuard::install(explicit);
    let mut effective = super::managed_config::effective_environment(&home)?;
    for entry in &mut effective {
        entry.origin = if explicit_keys.contains(&entry.key) {
            "process_or_explicit_option"
        } else if persisted_keys.contains(&entry.key) {
            "managed_file"
        } else {
            "default"
        };
    }
    if role == Role::Server {
        let mut planned = values.clone();
        super::firstrun::populate_env_defaults(&mut planned, &home.join("data"))?;
        let _validation = crate::config::PluginEnvGuard::install(planned.into_iter().collect());
        crate::config::Config::load_for_inspection().map_err(|e| {
            io::Error::other(format!(
                "invalid server configuration before deployment: {e}"
            ))
        })?;
    }
    let mut phases = input::credential_phases(role, &clients, oauth, mcp_token, &values)?;
    if options.dry_run {
        return Ok(StartReport {
            role,
            server,
            dry_run: true,
            effective,
            capabilities: detected,
            client_paths,
            phases,
            has_errors: false,
        });
    }
    // Show a redacted preview before writes in the interactive flow.
    if interactive {
        for entry in &effective {
            eprintln!("{}={} ({})", entry.key, entry.value, entry.origin);
        }
    }
    if role == Role::Server {
        let report = super::run_setup(super::SetupMode::Repair).await?;
        phases.extend(report.phases);
    } else {
        let checks = if role == Role::Client {
            super::verification::verify_requested_connection(
                &server,
                mcp_token,
                values.get("CORTEX_API_TOKEN").map(String::as_str),
                oauth,
                Duration::from_secs(10),
                !clients.is_empty()
                    || mcp_token.is_some()
                    || values.get("CORTEX_API_TOKEN").is_none_or(|v| v.is_empty()),
            )
            .await
        } else {
            vec![
                super::verification::verify_agent_auth(
                    &server,
                    values.get("CORTEX_HEARTBEAT_TOKEN").map(String::as_str),
                    super::auth_policy::enabled(
                        values
                            .get("CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP")
                            .map(String::as_str),
                    ),
                )
                .await,
            ]
        };
        phases.extend(checks);
        if role == Role::Agent && !clients.is_empty() && !super::phases_have_errors(&phases) {
            phases.extend(
                super::verification::verify_connection(
                    &server,
                    mcp_token,
                    values.get("CORTEX_API_TOKEN").map(String::as_str),
                    oauth,
                    Duration::from_secs(10),
                )
                .await,
            );
        }
        if !super::phases_have_errors(&phases) {
            super::firstrun::ensure_private_dir(&home)?;
            super::firstrun::write_env(&home.join(".env"), &values)?;
        }
    }
    if super::phases_have_errors(&phases) {
        return Ok(StartReport {
            role,
            server,
            dry_run: false,
            effective,
            capabilities: detected,
            client_paths,
            phases,
            has_errors: true,
        });
    }
    if role == Role::Agent
        || !selected.is_empty()
        || (role == Role::Server && home.join("heartbeat-agent.env").exists())
    {
        let path = home.join("heartbeat-agent.env");
        let mut agent = if path.exists() {
            super::heartbeat_agent_env::load_private_agent_env(&path)?
        } else {
            BTreeMap::new()
        };
        for (key, value) in &values {
            if (key.starts_with("CORTEX_AGENT_")
                && super::heartbeat_agent_env::RECOGNIZED_KEYS.contains(&key.as_str()))
                || matches!(
                    key.as_str(),
                    "CORTEX_HEARTBEAT_TOKEN" | "CORTEX_HEARTBEAT_TARGET" | "CORTEX_SYSLOG_TARGET"
                )
            {
                agent.insert(key.clone(), value.clone());
            }
        }
        agent
            .entry("CORTEX_HEARTBEAT_TARGET".into())
            .or_insert_with(|| server.clone());
        if !agent.contains_key("CORTEX_HEARTBEAT_TOKEN") {
            let installed = load_env(&home.join(".env"))?;
            if let Some(token) = installed.get("CORTEX_TOKEN") {
                agent.insert("CORTEX_HEARTBEAT_TOKEN".into(), token.clone());
            }
        }
        super::heartbeat_agent_env::write_private_agent_env(&path, &agent)?;
        if selected.iter().any(|s| s == "agent_commands") {
            let adapter = super::run_shell_agent_setup(super::ShellAgentAction::Install).await?;
            phases.extend(adapter.phases);
        }
        super::configure_agent_capabilities(&selected)?;
        if !super::phases_have_errors(&phases) {
            phases.push(
                super::verification::verify_agent_delivery(
                    &server,
                    agent.get("CORTEX_HEARTBEAT_TOKEN").map(String::as_str),
                    &home.join("heartbeat-host-id"),
                    super::auth_policy::enabled(
                        agent
                            .get("CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP")
                            .map(String::as_str),
                    ),
                )
                .await,
            );
        }
        if !super::phases_have_errors(&phases) {
            phases.extend(
                super::run_heartbeat_agent_setup(super::HeartbeatAgentAction::Install)
                    .await?
                    .phases,
            );
        }
    }
    let installed = load_env(&home.join(".env"))?;
    let static_disabled = oauth
        && installed
            .get("CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH")
            .is_none_or(|v| v != "false");
    if !super::phases_have_errors(&phases) {
        super::client_config::configure_clients(
            &user_home,
            &clients,
            &server,
            if static_disabled {
                None
            } else {
                installed.get("CORTEX_TOKEN").map(String::as_str)
            },
            false,
        )?;
        if role == Role::Server && (backup_schedule || options.backup_schedule == Some(false)) {
            let timer = PhaseTimer::start("backup-schedule");
            let action = if backup_schedule {
                crate::update::BackupAction::ScheduleInstall
            } else {
                crate::update::BackupAction::ScheduleRemove
            };
            let detail = crate::update::run_backup_action(action, Some(&home), None)?;
            phases.push(timer.finish(SetupStatus::Ok, detail));
        }
        let saved = SavedSetup {
            role: Some(role),
            server: Some(server.clone()),
            clients,
            capabilities: selected,
            backup_schedule,
        };
        super::heartbeat_agent_env::atomic_private_write(
            &saved_path,
            toml::to_string(&saved)
                .map_err(io::Error::other)?
                .as_bytes(),
            0o600,
        )?;
        if role == Role::Server {
            crate::update::configure_local_server_profile(None, &home)?;
        }
        phases.push(PhaseTimer::start("saved-setup").finish(
            SetupStatus::Ok,
            "role, clients, collection consent and deployment settings saved",
        ));
    }
    let capabilities = super::discover_agent_capabilities()?;
    let has_errors = super::phases_have_errors(&phases);
    Ok(StartReport {
        role,
        server,
        dry_run: false,
        effective,
        capabilities,
        client_paths,
        phases,
        has_errors,
    })
}

#[cfg(test)]
#[path = "onboarding_tests.rs"]
mod tests;
