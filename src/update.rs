pub(crate) mod lifecycle;
pub use lifecycle::{BackupAction, prepare_local_upgrade, run_backup_action};

use std::io::{self, ErrorKind};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::agent_deploy::{
    AgentDeployConfig, DeployResult, HostProbe, deploy_agent_to_host, find_local_binary,
    probe_hosts,
};
use crate::deploy::{RemoteDeployOptions, RemoteDeployReport, run_remote_deploy};
use crate::setup::{PhaseTimer, SetupPhase, SetupStatus, cortex_home_dir};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateScope {
    All,
    Server,
    Clients,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    pub dry_run: bool,
    pub profile_path: Option<PathBuf>,
    pub binary: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerUpdateProfile {
    pub host: String,
    pub home: String,
    #[serde(default)]
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClientsUpdateProfile {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub docker: Option<bool>,
    #[serde(default)]
    pub journald: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdateProfile {
    #[serde(default)]
    pub server: Option<ServerUpdateProfile>,
    #[serde(default)]
    pub clients: ClientsUpdateProfile,
}

pub fn default_profile_path() -> io::Result<PathBuf> {
    Ok(cortex_home_dir()?.join("deployments.toml"))
}

pub fn load_profile(path: &Path) -> io::Result<UpdateProfile> {
    match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str(&raw).map_err(|error| {
            io::Error::new(
                ErrorKind::InvalidData,
                format!("parse update profile {}: {error}", path.display()),
            )
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(UpdateProfile::default()),
        Err(error) => Err(error),
    }
}

pub fn write_profile(path: &Path, profile: &UpdateProfile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rendered = toml::to_string_pretty(profile).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("render update profile {}: {error}", path.display()),
        )
    })?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, rendered)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn configure_server_profile(
    path: Option<&Path>,
    host: &str,
    home: &str,
) -> io::Result<UpdateProfile> {
    let path = resolve_profile_path(path)?;
    let mut profile = load_profile(&path)?;
    profile.server = Some(ServerUpdateProfile {
        host: validate_host(host)?,
        home: validate_remote_home(home)?,
        local: false,
    });
    write_profile(&path, &profile)?;
    Ok(profile)
}

/// Remember a completed local installation without changing fleet clients.
pub fn configure_local_server_profile(
    path: Option<&Path>,
    home: &Path,
) -> io::Result<UpdateProfile> {
    let path = resolve_profile_path(path)?;
    let mut profile = load_profile(&path)?;
    profile.server = Some(ServerUpdateProfile {
        host: "localhost".to_string(),
        home: validate_remote_home(&home.to_string_lossy())?,
        local: true,
    });
    write_profile(&path, &profile)?;
    Ok(profile)
}

pub fn configure_clients_profile(
    path: Option<&Path>,
    hosts: Vec<String>,
    target: Option<String>,
    docker: Option<bool>,
    journald: Option<bool>,
) -> io::Result<UpdateProfile> {
    let path = resolve_profile_path(path)?;
    let mut validated = Vec::new();
    for host in hosts {
        validated.push(validate_host(&host)?);
    }
    if validated.is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "at least one client host is required",
        ));
    }
    let mut profile = load_profile(&path)?;
    let target = match target {
        Some(target) => Some(validate_client_target(&target)?),
        None => profile.clients.target.clone(),
    };
    profile.clients = ClientsUpdateProfile {
        hosts: validated,
        target,
        docker: docker.or(profile.clients.docker),
        journald: journald.or(profile.clients.journald),
    };
    write_profile(&path, &profile)?;
    Ok(profile)
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateReport {
    pub mode: &'static str,
    pub profile_path: PathBuf,
    pub server: Option<RemoteDeployReport>,
    pub clients: Vec<DeployResult>,
    pub skipped: Vec<SetupPhase>,
    pub has_errors: bool,
    pub elapsed_ms: u128,
}

trait UpdateRunner {
    fn run_server(
        &mut self,
        host: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport>;

    fn run_local_server(
        &mut self,
        home: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport> {
        crate::deploy::run_local_deploy(home, options)
    }

    fn deploy_client(
        &mut self,
        host: &str,
        binary: &Path,
        config: &AgentDeployConfig,
    ) -> DeployResult;

    fn find_binary(&self) -> Option<PathBuf>;

    fn probe_clients(&mut self, hosts: Vec<String>) -> Vec<HostProbe>;

    fn validate_binary(&self, path: &Path) -> io::Result<()> {
        validate_agent_binary(path)
    }
}

struct RealUpdateRunner;

impl UpdateRunner for RealUpdateRunner {
    fn run_server(
        &mut self,
        host: &str,
        options: RemoteDeployOptions,
    ) -> io::Result<RemoteDeployReport> {
        run_remote_deploy(host, options)
    }

    fn deploy_client(
        &mut self,
        host: &str,
        binary: &Path,
        config: &AgentDeployConfig,
    ) -> DeployResult {
        deploy_agent_to_host(host, binary, config)
    }

    fn find_binary(&self) -> Option<PathBuf> {
        find_local_binary()
    }

    fn probe_clients(&mut self, hosts: Vec<String>) -> Vec<HostProbe> {
        probe_hosts(hosts)
    }
}

pub fn run_update(scope: UpdateScope, options: UpdateOptions) -> io::Result<UpdateReport> {
    let mut runner = RealUpdateRunner;
    run_update_with_runner(scope, options, &mut runner)
}

fn run_update_with_runner(
    scope: UpdateScope,
    options: UpdateOptions,
    runner: &mut dyn UpdateRunner,
) -> io::Result<UpdateReport> {
    let started = Instant::now();
    let profile_path = resolve_profile_path(options.profile_path.as_deref())?;
    let profile = validate_loaded_profile(load_profile(&profile_path)?)?;
    let mut server = None;
    let mut clients = Vec::new();
    let mut skipped = Vec::new();

    if matches!(scope, UpdateScope::All | UpdateScope::Server) {
        let target = profile.server.as_ref().ok_or_else(|| {
            io::Error::new(
                ErrorKind::NotFound,
                format!(
                    "no server update profile at {}; run `cortex update config server --host HOST --home PATH`",
                    profile_path.display()
                ),
            )
        })?;
        let deploy_options = RemoteDeployOptions {
            dry_run: options.dry_run,
            home: Some(target.home.clone()),
            update_latest: true,
        };
        let report = if target.local {
            runner.run_local_server(&target.home, deploy_options)?
        } else {
            runner.run_server(&target.host, deploy_options)?
        };
        let failed = report.has_errors;
        server = Some(report);
        if failed && matches!(scope, UpdateScope::All) {
            skipped.push(
                PhaseTimer::start("clients")
                    .finish(SetupStatus::Skipped, "skipped because server update failed"),
            );
            return Ok(build_report(
                scope,
                profile_path,
                server,
                clients,
                skipped,
                started,
            ));
        }
    }

    if matches!(scope, UpdateScope::All | UpdateScope::Clients) {
        if profile.clients.hosts.is_empty() {
            if matches!(scope, UpdateScope::Clients) {
                return Err(io::Error::new(
                    ErrorKind::NotFound,
                    format!(
                        "no client update profile at {}; run `cortex update config clients --hosts HOST1,HOST2`",
                        profile_path.display()
                    ),
                ));
            }
            skipped.push(
                PhaseTimer::start("clients")
                    .finish(SetupStatus::Skipped, "no configured client hosts"),
            );
        } else if options.dry_run {
            let _binary = resolve_agent_binary(&options, runner)?;
            let probes = runner.probe_clients(profile.clients.hosts.clone());
            clients.extend(client_preflight_results(&profile.clients.hosts, probes));
            skipped.push(PhaseTimer::start("clients").finish(
                SetupStatus::Skipped,
                "dry-run probed client agents without deploying",
            ));
        } else {
            let binary = resolve_agent_binary(&options, runner)?;
            let config = AgentDeployConfig {
                target: profile.clients.target.clone(),
                token: None,
                require_token: true,
                docker: profile.clients.docker,
                journald: profile.clients.journald,
            };
            for host in &profile.clients.hosts {
                clients.push(runner.deploy_client(host, &binary, &config));
            }
        }
    }

    Ok(build_report(
        scope,
        profile_path,
        server,
        clients,
        skipped,
        started,
    ))
}

fn resolve_agent_binary(options: &UpdateOptions, runner: &dyn UpdateRunner) -> io::Result<PathBuf> {
    let binary = options
        .binary
        .clone()
        .or_else(|| runner.find_binary())
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "cortex binary not found"))?;
    runner.validate_binary(&binary)?;
    Ok(binary)
}

fn validate_agent_binary(path: &Path) -> io::Result<()> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cortex binary {} is not usable: {error}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("cortex binary {} is not a file", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                format!("cortex binary {} is not executable", path.display()),
            ));
        }
    }
    Ok(())
}

fn client_preflight_results(hosts: &[String], probes: Vec<HostProbe>) -> Vec<DeployResult> {
    let mut results = Vec::with_capacity(hosts.len());
    for host in hosts {
        match probes.iter().find(|probe| probe.host == *host) {
            Some(probe) => results.push(DeployResult {
                host: host.clone(),
                ok: probe.reachable,
                detail: probe.display_label(),
                elapsed_ms: 0,
            }),
            None => results.push(DeployResult {
                host: host.clone(),
                ok: false,
                detail: "dry-run probe timed out".to_string(),
                elapsed_ms: 0,
            }),
        }
    }
    results
}

fn build_report(
    scope: UpdateScope,
    profile_path: PathBuf,
    server: Option<RemoteDeployReport>,
    clients: Vec<DeployResult>,
    skipped: Vec<SetupPhase>,
    started: Instant,
) -> UpdateReport {
    let has_errors = server.as_ref().is_some_and(|report| report.has_errors)
        || clients.iter().any(|result| !result.ok)
        || skipped
            .iter()
            .any(|phase| matches!(phase.status, SetupStatus::Error));
    UpdateReport {
        mode: match scope {
            UpdateScope::All => "all",
            UpdateScope::Server => "server",
            UpdateScope::Clients => "clients",
        },
        profile_path,
        server,
        clients,
        skipped,
        has_errors,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

fn validate_loaded_profile(mut profile: UpdateProfile) -> io::Result<UpdateProfile> {
    if let Some(server) = &mut profile.server {
        server.host = validate_host(&server.host)?;
        server.home = validate_remote_home(&server.home)?;
    }
    profile.clients.hosts = profile
        .clients
        .hosts
        .iter()
        .map(|host| validate_host(host))
        .collect::<io::Result<Vec<_>>>()?;
    if let Some(target) = profile.clients.target.take() {
        profile.clients.target = Some(validate_client_target(&target)?);
    }
    Ok(profile)
}

fn resolve_profile_path(path: Option<&Path>) -> io::Result<PathBuf> {
    match path {
        Some(path) => Ok(path.to_path_buf()),
        None => default_profile_path(),
    }
}

fn validate_host(host: &str) -> io::Result<String> {
    let trimmed = host.trim();
    if trimmed.is_empty() || !crate::inventory::ssh::is_safe_ssh_host(trimmed) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("unsafe ssh host: {host}"),
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_client_target(target: &str) -> io::Result<String> {
    let trimmed = target.trim();
    let parsed = Url::parse(trimmed).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("client target must be an http(s) URL: {error}"),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "client target must be an http(s) URL with a host",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_remote_home(home: &str) -> io::Result<String> {
    let trimmed = home.trim();
    let path = Path::new(trimmed);
    if trimmed.is_empty()
        || trimmed.contains(['\0', '\n', '\r'])
        || trimmed == "/"
        || !path.is_absolute()
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "server home must be a non-empty absolute path",
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "server home must not contain '..'",
        ));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
