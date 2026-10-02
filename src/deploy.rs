pub(crate) mod lifecycle_lock;
mod release;
mod remote;
mod remote_assets;
mod remote_support;
mod verification;

pub use remote::{RemoteDeployOptions, RemoteDeployReport, run_local_deploy, run_remote_deploy};

/// Verify an explicitly restored managed target without changing credentials or
/// automatically rolling its database back after a migration.
pub(crate) fn verify_local_managed_server(
    home: &std::path::Path,
) -> std::io::Result<Vec<crate::setup::SetupPhase>> {
    let mut runner = remote_support::LocalRemoteRunner;
    let home = home.to_string_lossy();
    let env = crate::setup::parse_env(&std::fs::read_to_string(
        std::path::Path::new(home.as_ref()).join(".env"),
    )?);
    let seconds = env
        .get("CORTEX_SETUP_READY_TIMEOUT_SECS")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(300);
    let phase = verification::readiness_phase(&mut runner, "localhost", &home, seconds)?;
    if matches!(phase.status, crate::setup::SetupStatus::Error) {
        return Ok(vec![phase]);
    }
    let mut phases = vec![phase];
    phases.extend(verification::verify_remote_server(
        &mut runner,
        "localhost",
        &home,
        &env,
    )?);
    Ok(phases)
}

/// Execute a task-owned script through the same bounded process-group runner
/// used by local deployment and SSH operations.
pub(crate) fn run_local_script(script: &str) -> std::io::Result<String> {
    use remote_support::RemoteRunner;
    let output = remote_support::LocalRemoteRunner.run("localhost", script, None)?;
    if !output.status_success {
        return Err(std::io::Error::other(format!(
            "managed lifecycle subprocess failed: {}",
            output.stderr.trim()
        )));
    }
    Ok(output.stdout.trim().to_string())
}
