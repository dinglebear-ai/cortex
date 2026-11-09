use std::io;
use std::path::Path;
use std::time::Instant;

use serde::Serialize;

use super::remote_support::{
    LocalRemoteRunner, RemoteRunner, SshRemoteRunner, append_skipped, phases_have_errors,
    remote_identity_phase, remote_phase, shell_quote, skip_phase,
};
use crate::setup::{SetupPhase, SetupStatus, default_env_for_data_dir, parse_env};

use super::remote_assets::{
    compose_command, managed_lifecycle_script, read_existing_remote_env,
    validate_remote_absolute_path, validate_remote_home, write_remote_assets_phase,
    write_remote_env_phase,
};

const REMOTE_HOME_SUFFIX: &str = ".cortex";

#[derive(Debug, Clone, Default)]
pub struct RemoteDeployOptions {
    pub dry_run: bool,
    pub home: Option<String>,
    /// Used by `cortex update`; setup/repair keep their shipped baseline.
    pub update_latest: bool,
}

impl From<bool> for RemoteDeployOptions {
    fn from(dry_run: bool) -> Self {
        Self {
            dry_run,
            home: None,
            update_latest: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteDeployReport {
    pub mode: &'static str,
    pub host: String,
    pub home: String,
    pub env_path: String,
    pub compose_dir: String,
    pub data_dir: String,
    pub health_url: String,
    pub mcp_url: String,
    pub phases: Vec<SetupPhase>,
    pub has_errors: bool,
    pub elapsed_ms: u128,
}

pub fn run_remote_deploy(
    host: &str,
    options: impl Into<RemoteDeployOptions>,
) -> io::Result<RemoteDeployReport> {
    let mut runner = SshRemoteRunner;
    let report = run_remote_deploy_with_runner(host, options, &mut runner)?;
    if !report.has_errors && report.mode != "remote dry-run" {
        crate::update::configure_server_profile(None, host, &report.home)?;
    }
    Ok(report)
}

pub fn run_local_deploy(
    home: &str,
    mut options: RemoteDeployOptions,
) -> io::Result<RemoteDeployReport> {
    options.home = Some(home.to_string());
    let mut runner = LocalRemoteRunner;
    let mut report = run_remote_deploy_with_runner("localhost", options, &mut runner)?;
    report.mode = if report.mode == "remote dry-run" {
        "local dry-run"
    } else {
        "local"
    };
    Ok(report)
}

fn run_remote_deploy_with_runner(
    host: &str,
    options: impl Into<RemoteDeployOptions>,
    runner: &mut dyn RemoteRunner,
) -> io::Result<RemoteDeployReport> {
    let options = options.into();
    let dry_run = options.dry_run;
    if !crate::inventory::ssh::is_safe_ssh_host(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe ssh host: {host}"),
        ));
    }
    let remote_home_override = options
        .home
        .as_deref()
        .map(validate_remote_home)
        .transpose()?;
    let started = Instant::now();
    let mut phases = Vec::new();

    let ssh_phase = remote_phase(runner, host, "ssh", "true", None)?;
    phases.push(ssh_phase);

    let identity = remote_identity_phase(runner, host)?;
    let identity_values = identity.values;
    phases.push(identity.phase);
    let Some(identity_values) = identity_values else {
        append_skipped(
            &mut phases,
            &[
                "remote-filesystem",
                "remote-env",
                "remote-compose-assets",
                "remote-docker",
                "remote-docker-network",
                "remote-compose-pull",
                "remote-compose-up",
                "remote-health",
            ],
            "skipped because remote identity failed",
        );
        return Ok(report(
            host,
            dry_run,
            "$HOME/.cortex",
            "$HOME/.cortex/data",
            "3100",
            phases,
            started.elapsed().as_millis(),
        ));
    };
    let remote_home = remote_home_override
        .unwrap_or_else(|| format!("{}/{}", identity_values.home, REMOTE_HOME_SUFFIX));
    let mut lifecycle =
        super::lifecycle_lock::LockedRunner::acquire(runner, host, &remote_home, !dry_run)?;
    let runner: &mut dyn RemoteRunner = &mut lifecycle;
    let remote_data_dir = format!("{remote_home}/data");
    let mut env = default_env_for_data_dir(Path::new(&remote_data_dir))?;
    env.insert("CORTEX_DATA_VOLUME".to_string(), remote_data_dir.clone());
    env.insert(
        "CORTEX_BACKUP_DIR".to_string(),
        format!("{remote_home}/backups"),
    );
    env.insert("CORTEX_UID".to_string(), identity_values.uid.clone());
    env.insert("CORTEX_GID".to_string(), identity_values.gid.clone());
    // Dry runs inspect the same saved configuration and version pin as an
    // actual deployment. Reading a file never executes its contents.
    let existing_env = read_existing_remote_env(runner, host, &remote_home)?;
    for (key, value) in parse_env(&existing_env) {
        env.insert(key, value);
    }
    let selected_version = if options.update_latest
        && !env
            .get("CORTEX_VERSION")
            .is_some_and(|value| !value.is_empty())
    {
        Some(runner.latest_stable_version()?)
    } else {
        None
    };
    if let Some(version) = &selected_version {
        phases.push(crate::setup::PhaseTimer::start("release-target").finish(
            SetupStatus::Ok,
            format!("latest stable release {version}; no explicit version pin changed"),
        ));
    }
    let docker_network = env
        .get("DOCKER_NETWORK")
        .cloned()
        .unwrap_or_else(|| "cortex".to_string());
    let mcp_port = env
        .get("CORTEX_PORT")
        .cloned()
        .unwrap_or_else(|| "3100".to_string());
    let remote_backup_dir = env
        .get("CORTEX_BACKUP_DIR")
        .expect("remote backup default is always populated");
    validate_remote_absolute_path(remote_backup_dir, "CORTEX_BACKUP_DIR")?;

    if dry_run {
        phases.push(remote_phase(
            runner,
            host,
            "remote-filesystem",
            &format!(
                "test -d {home} || test -w $(dirname {home})",
                home = shell_quote(&remote_home)
            ),
            None,
        )?);
        phases.push(remote_phase(
            runner,
            host,
            "remote-docker",
            "docker --version && docker compose version",
            None,
        )?);
        let dry_run_reason = "dry-run does not mutate remote Docker or files";
        phases.push(skip_phase("remote-env", dry_run_reason));
        phases.push(skip_phase("remote-compose-assets", dry_run_reason));
        phases.push(skip_phase(
            "remote-docker-network",
            "dry-run does not create Docker networks",
        ));
        phases.push(skip_phase(
            "remote-compose-pull",
            "dry-run does not pull images",
        ));
        phases.push(skip_phase(
            "remote-compose-up",
            "dry-run does not start Docker services",
        ));
        phases.push(skip_phase(
            "remote-health",
            "dry-run does not check service health",
        ));
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    phases.push(remote_phase(
        runner,
        host,
        "remote-recovery-snapshot",
        &managed_lifecycle_script("prepare", &remote_home, &env),
        None,
    )?);
    if phases_have_errors(&phases) {
        append_skipped(
            &mut phases,
            &[
                "remote-env",
                "remote-compose-assets",
                "remote-compose-up",
                "remote-health",
            ],
            "skipped because the owned deployment recovery snapshot failed",
        );
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    phases.push(remote_phase(
        runner,
        host,
        "remote-filesystem",
        &format!(
            "mkdir -p {compose_config} {data_dir} {backup_dir} && chmod 700 {backup_dir}",
            compose_config = shell_quote(&format!("{remote_home}/compose/config")),
            data_dir = shell_quote(&remote_data_dir),
            backup_dir = shell_quote(remote_backup_dir),
        ),
        None,
    )?);
    phases.push(remote_phase(
        runner,
        host,
        "remote-docker",
        "docker --version && docker compose version",
        None,
    )?);
    if phases_have_errors(&phases) {
        append_skipped(
            &mut phases,
            &[
                "remote-env",
                "remote-compose-assets",
                "remote-docker-network",
                "remote-compose-pull",
                "remote-compose-up",
                "remote-health",
            ],
            "skipped because remote prerequisites failed",
        );
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    phases.push(write_remote_env_phase(
        runner,
        host,
        &remote_home,
        &env,
        &existing_env,
    )?);
    phases.push(write_remote_assets_phase(
        runner,
        host,
        &remote_home,
        selected_version.as_deref(),
    )?);
    if phases_have_errors(&phases) {
        append_skipped(
            &mut phases,
            &[
                "remote-docker-network",
                "remote-compose-pull",
                "remote-compose-up",
                "remote-health",
            ],
            "skipped because remote asset setup failed",
        );
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    phases.push(remote_phase(
        runner,
        host,
        "remote-docker-network",
        &format!(
            "docker network inspect {network} >/dev/null 2>&1 || docker network create {network}",
            network = shell_quote(&docker_network)
        ),
        None,
    )?);
    if phases_have_errors(&phases) {
        append_skipped(
            &mut phases,
            &["remote-compose-pull", "remote-compose-up", "remote-health"],
            "skipped because Docker network setup failed",
        );
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    phases.push(remote_phase(
        runner,
        host,
        "remote-compose-pull",
        &format!(
            "{}\ncompose pull --ignore-buildable",
            compose_command(&remote_home),
        ),
        None,
    )?);
    if phases_have_errors(&phases) {
        append_skipped(
            &mut phases,
            &["remote-compose-up", "remote-health"],
            "skipped because Compose pull failed",
        );
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    phases.push(remote_phase(
        runner,
        host,
        "remote-compose-up",
        &format!("{}\ncompose up -d cortex", compose_command(&remote_home),),
        None,
    )?);
    if phases_have_errors(&phases) {
        append_skipped(
            &mut phases,
            &["remote-health"],
            "skipped because Compose up failed",
        );
        return Ok(report(
            host,
            dry_run,
            &remote_home,
            &remote_data_dir,
            &mcp_port,
            phases,
            started.elapsed().as_millis(),
        ));
    }

    let ready_seconds = env
        .get("CORTEX_SETUP_READY_TIMEOUT_SECS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(300)
        .clamp(1, 600);
    phases.push(super::verification::readiness_phase(
        runner,
        host,
        &remote_home,
        ready_seconds,
    )?);
    if !phases_have_errors(&phases) {
        phases.extend(super::verification::verify_remote_server(
            runner,
            host,
            &remote_home,
            &env,
        )?);
    }
    Ok(report(
        host,
        dry_run,
        &remote_home,
        &remote_data_dir,
        &mcp_port,
        phases,
        started.elapsed().as_millis(),
    ))
}

fn report(
    host: &str,
    dry_run: bool,
    remote_home: &str,
    remote_data_dir: &str,
    mcp_port: &str,
    phases: Vec<SetupPhase>,
    elapsed_ms: u128,
) -> RemoteDeployReport {
    let has_errors = phases
        .iter()
        .any(|phase| matches!(phase.status, SetupStatus::Error));
    RemoteDeployReport {
        mode: if dry_run { "remote dry-run" } else { "remote" },
        host: host.to_string(),
        home: remote_home.to_string(),
        env_path: format!("{remote_home}/.env"),
        compose_dir: format!("{remote_home}/compose"),
        data_dir: remote_data_dir.to_string(),
        health_url: format!("http://127.0.0.1:{mcp_port}/health"),
        mcp_url: format!("http://127.0.0.1:{mcp_port}/mcp"),
        phases,
        has_errors,
        elapsed_ms,
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
