use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Server,
    Client,
    Agent,
}
impl Role {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value {
            "server" => Ok(Self::Server),
            "client" => Ok(Self::Client),
            "agent" => Ok(Self::Agent),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "role must be server, client, or agent",
            )),
        }
    }
}
#[derive(Default, Clone, PartialEq, Eq)]
pub struct StartOptions {
    pub role: Option<Role>,
    pub server: Option<String>,
    pub token_file: Option<PathBuf>,
    pub api_token_file: Option<PathBuf>,
    pub clients: Option<Vec<String>>,
    pub capabilities: Option<Vec<String>>,
    pub settings: BTreeMap<String, String>,
    pub backup_schedule: Option<bool>,
    pub dry_run: bool,
    pub interactive: bool,
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct SavedSetup {
    pub(super) role: Option<Role>,
    pub(super) server: Option<String>,
    #[serde(default)]
    pub(super) clients: Vec<String>,
    #[serde(default)]
    pub(super) capabilities: Vec<String>,
    #[serde(default)]
    pub(super) backup_schedule: bool,
}
pub fn read_secret(path: &Path) -> io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "secret must be a regular non-symlink file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "secret file must be owned by this user with mode 0600 or narrower",
            ));
        }
    }
    let value = std::fs::read_to_string(path)?
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "secret file must contain one non-empty line",
        ));
    }
    Ok(value)
}

pub(super) fn prompt(label: &str, default: &str) -> io::Result<String> {
    eprint!("{label} [{default}]: ");
    io::stderr().flush()?;
    let mut value = String::new();
    if io::stdin().read_line(&mut value)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "setup input ended; use explicit options for unattended setup",
        ));
    }
    let value = value.trim();
    Ok(if value.is_empty() {
        default.into()
    } else {
        value.into()
    })
}
pub(super) fn csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
        .map(str::to_string)
        .collect()
}
pub(super) fn load_env(path: &Path) -> io::Result<BTreeMap<String, String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(super::super::firstrun::parse_env(&raw)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

/// Retained credentials alone do not mean the removed collector is installed.
pub(super) fn restore_agent_enrollment(
    home: &Path,
    values: &mut BTreeMap<String, String>,
) -> io::Result<()> {
    let path = home.join("heartbeat-agent.env");
    if path.exists() {
        for (key, value) in super::super::heartbeat_agent_env::load_private_agent_env(&path)? {
            values.entry(key).or_insert(value);
        }
    }
    Ok(())
}

pub(super) fn prompt_server_settings(values: &mut BTreeMap<String, String>) -> io::Result<()> {
    for (key, label, default) in [
        (
            "CORTEX_AUTH_MODE",
            "Authentication (bearer/oauth)",
            "bearer",
        ),
        (
            "CORTEX_MCP_BIND",
            "MCP/API publication address (loopback or trusted network)",
            "127.0.0.1",
        ),
        (
            "CORTEX_ALLOWED_SOURCE_CIDRS",
            "Allowed syslog sender CIDRs (blank requires network access controls)",
            "",
        ),
        ("CORTEX_MAX_DB_SIZE_MB", "Database budget in MiB", "8192"),
        ("CORTEX_RETENTION_DAYS", "Retention in days", "90"),
    ] {
        let value = prompt(
            label,
            values.get(key).map(String::as_str).unwrap_or(default),
        )?;
        if !value.is_empty() {
            super::super::managed_config::validate_setting(key, &value)?;
            values.insert(key.into(), value);
        }
    }
    if values.get("CORTEX_AUTH_MODE").is_some_and(|v| v == "oauth") {
        for (key, label) in [
            ("CORTEX_PUBLIC_URL", "Public HTTPS URL"),
            ("CORTEX_GOOGLE_CLIENT_ID", "Google OAuth client ID"),
            ("CORTEX_AUTH_ADMIN_EMAIL", "OAuth administrator email"),
        ] {
            if values.get(key).is_none_or(|v| v.is_empty()) {
                values.insert(key.into(), prompt(label, "")?);
            }
        }
        if values
            .get("CORTEX_GOOGLE_CLIENT_SECRET")
            .is_none_or(|v| v.is_empty())
        {
            values.insert(
                "CORTEX_GOOGLE_CLIENT_SECRET".into(),
                read_secret(Path::new(&prompt("Private Google OAuth secret file", "")?))?,
            );
        }
    }
    Ok(())
}

pub(super) fn confirm_credential_transport(
    interactive: bool,
    role: Role,
    server: &str,
    values: &mut BTreeMap<String, String>,
) -> io::Result<()> {
    if interactive
        && role != Role::Server
        && super::super::verification::validate_credential_transport(server, false).is_err()
        && !super::super::auth_policy::enabled(
            values
                .get("CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP")
                .map(String::as_str),
        )
    {
        if matches!(
            prompt(
                "Permit credentials over HTTP on this trusted overlay network? (yes/no)",
                "no"
            )?
            .as_str(),
            "yes" | "y"
        ) {
            values.insert(
                "CORTEX_AGENT_ALLOW_TRUSTED_OVERLAY_HTTP".into(),
                "true".into(),
            );
        } else {
            return Err(io::Error::other(
                "choose an HTTPS server URL or explicitly permit trusted-overlay HTTP",
            ));
        }
    }

    Ok(())
}

pub(super) fn complete_client_credentials(
    role: Role,
    clients: &[String],
    interactive: bool,
    dry_run: bool,
    oauth: bool,
    values: &mut BTreeMap<String, String>,
) -> io::Result<()> {
    if role == Role::Server {
        return Ok(());
    }
    if !clients.is_empty()
        && !oauth
        && !super::super::auth_policy::no_auth(values)?
        && values.get("CORTEX_TOKEN").is_none_or(|v| v.is_empty())
    {
        if interactive {
            values.insert(
                "CORTEX_TOKEN".into(),
                read_secret(Path::new(&prompt(
                    "Private MCP token file for selected clients",
                    "",
                )?))?,
            );
        } else if !dry_run {
            return Err(io::Error::other(
                "configuring MCP clients requires --secret-file CORTEX_TOKEN=PATH (agent --token-file enrolls ingestion only)",
            ));
        }
    }
    if interactive && values.get("CORTEX_API_TOKEN").is_none_or(|v| v.is_empty()) {
        let path = prompt(
            "Private REST API token file (blank leaves CLI REST pending)",
            "",
        )?;
        if !path.is_empty() {
            values.insert("CORTEX_API_TOKEN".into(), read_secret(Path::new(&path))?);
        }
    }
    Ok(())
}

pub(super) fn credential_phases(
    role: Role,
    clients: &[String],
    oauth: bool,
    mcp_token: Option<&str>,
    values: &BTreeMap<String, String>,
) -> io::Result<Vec<super::SetupPhase>> {
    let mut phases = Vec::new();
    let no_auth = super::super::auth_policy::no_auth(values)?;
    if role != Role::Server && !clients.is_empty() && !oauth && mcp_token.is_none() && !no_auth {
        phases.push(super::PhaseTimer::start("mcp-credential").finish(
            super::SetupStatus::Warn,
            "needs_configuration: provide MCP credentials for the selected clients",
        ));
    }
    if role != Role::Server && values.get("CORTEX_API_TOKEN").is_none_or(|v| v.is_empty()) {
        phases.push(super::PhaseTimer::start("rest-credential").finish(super::SetupStatus::Warn,
            "needs_configuration: CLI REST requires a separate API token; supply --api-token-file when needed"));
    }
    Ok(phases)
}
