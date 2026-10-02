use super::remote_support::{RemoteRunner, remote_phase, shell_quote};
use crate::setup::{SetupPhase, dockerfile_asset, installed_compose_asset};
use std::io;
use std::path::Path;

pub(super) fn write_remote_env_phase(
    runner: &mut dyn RemoteRunner,
    host: &str,
    remote_home: &str,
    env: &std::collections::BTreeMap<String, String>,
    existing_env: &str,
) -> io::Result<SetupPhase> {
    let rendered = crate::setup::dotenv::render_preserving(existing_env, env);
    let env_path = format!("{remote_home}/.env");
    let tmp_path = format!("{env_path}.tmp");
    let legacy_env_path = format!("{remote_home}/compose/.env");
    let legacy_archive_path = format!("{remote_home}/compose/.env.legacy");
    let script = format!(
        "set -eu\numask 077\ncat > {tmp_path} <<'__CORTEX_ENV__'\n{rendered}__CORTEX_ENV__\nchmod 600 {tmp_path}\nmv {tmp_path} {env_path}\nif test -f {legacy_env_path}; then rm -f {legacy_archive_path}; mv {legacy_env_path} {legacy_archive_path}; chmod 600 {legacy_archive_path}; fi",
        tmp_path = shell_quote(&tmp_path),
        env_path = shell_quote(&env_path),
        legacy_env_path = shell_quote(&legacy_env_path),
        legacy_archive_path = shell_quote(&legacy_archive_path),
    );
    remote_phase(runner, host, "remote-env", &script, None)
}

pub(super) fn write_remote_assets_phase(
    runner: &mut dyn RemoteRunner,
    host: &str,
    remote_home: &str,
    selected_version: Option<&str>,
) -> io::Result<SetupPhase> {
    let compose_asset = match selected_version {
        Some(version) => installed_compose_asset().replace(
            &format!("${{CORTEX_VERSION:-{}}}", env!("CARGO_PKG_VERSION")),
            &format!("${{CORTEX_VERSION:-{version}}}"),
        ),
        None => installed_compose_asset(),
    };
    let compose_path = format!("{remote_home}/compose/docker-compose.yml");
    let compose_tmp = format!("{compose_path}.tmp");
    let dockerfile_path = format!("{remote_home}/compose/config/Dockerfile");
    let dockerfile_tmp = format!("{dockerfile_path}.tmp");
    let lifecycle_path = format!("{remote_home}/managed-lifecycle.sh");
    let lifecycle_tmp = format!("{lifecycle_path}.tmp");
    let script = format!(
        "set -eu\numask 077\ncat > {lifecycle_tmp} <<'__CORTEX_LIFECYCLE__'\n{}__CORTEX_LIFECYCLE__\ncat > {compose_tmp} <<'__CORTEX_COMPOSE__'\n{}__CORTEX_COMPOSE__\ncat > {dockerfile_tmp} <<'__CORTEX_DOCKERFILE__'\n{}__CORTEX_DOCKERFILE__\nchmod 700 {lifecycle_tmp}\nmv {compose_tmp} {compose_path}\nmv {dockerfile_tmp} {dockerfile_path}\nmv {lifecycle_tmp} {lifecycle_path}",
        crate::update::lifecycle::MANAGED_LIFECYCLE,
        compose_asset,
        dockerfile_asset(),
        lifecycle_tmp = shell_quote(&lifecycle_tmp),
        lifecycle_path = shell_quote(&lifecycle_path),
        compose_tmp = shell_quote(&compose_tmp),
        compose_path = shell_quote(&compose_path),
        dockerfile_tmp = shell_quote(&dockerfile_tmp),
        dockerfile_path = shell_quote(&dockerfile_path),
    );
    remote_phase(runner, host, "remote-compose-assets", &script, None)
}

pub(super) fn read_existing_remote_env(
    runner: &mut dyn RemoteRunner,
    host: &str,
    remote_home: &str,
) -> io::Result<String> {
    let env_path = format!("{remote_home}/.env");
    let legacy_env_path = format!("{remote_home}/compose/.env");
    let script = format!(
        "if test -f {env_path}; then cat {env_path}; elif test -f {legacy_env_path}; then cat {legacy_env_path}; fi",
        env_path = shell_quote(&env_path),
        legacy_env_path = shell_quote(&legacy_env_path),
    );
    let output = runner.run(host, &script, None)?;
    if !output.status_success {
        return Err(io::Error::other(
            "failed to read existing remote environment",
        ));
    }
    Ok(output.stdout)
}

pub(super) fn validate_remote_home(home: &str) -> io::Result<String> {
    let trimmed = home.trim();
    if trimmed.is_empty() || trimmed.contains(['\0', '\n', '\r']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote deploy --home must be a non-empty single-line absolute path",
        ));
    }
    if !Path::new(trimmed).is_absolute()
        || trimmed == "/"
        || Path::new(trimmed)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote deploy --home must be an absolute path",
        ));
    }
    Ok(trimmed.to_string())
}

pub(super) fn validate_remote_absolute_path(path: &str, name: &str) -> io::Result<()> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.contains(['\0', '\n', '\r'])
        || !Path::new(trimmed).is_absolute()
        || Path::new(trimmed)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        || trimmed == "/"
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("remote deploy {name} must be a safe single-line absolute path"),
        ));
    }
    Ok(())
}

pub(super) fn managed_lifecycle_script(
    action: &str,
    home: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> String {
    let policy = ["CORTEX_BACKUP_RETAIN_COUNT", "CORTEX_BACKUP_MIN_FREE_MB"]
        .into_iter()
        .filter_map(|key| {
            env.get(key)
                .map(|value| format!("export {key}={}\n", shell_quote(value)))
        })
        .collect::<String>();
    format!(
        "{policy}set -- {} {}\n{}",
        shell_quote(action),
        shell_quote(home),
        crate::update::lifecycle::MANAGED_LIFECYCLE
    )
}

pub(super) fn compose_command(home: &str) -> String {
    format!(
        r#"set -eu
home={home}
compose() {{
  if test -f "$home/compose/docker-compose.recovery.yml"; then
    if test -f "$home/compose/docker-compose.override.yml"; then
      docker compose --project-directory "$home/compose" --env-file "$home/.env" -f "$home/compose/docker-compose.yml" -f "$home/compose/docker-compose.override.yml" -f "$home/compose/docker-compose.recovery.yml" "$@"
    else
      docker compose --project-directory "$home/compose" --env-file "$home/.env" -f "$home/compose/docker-compose.yml" -f "$home/compose/docker-compose.recovery.yml" "$@"
    fi
  elif test -f "$home/compose/docker-compose.override.yml"; then
    docker compose --project-directory "$home/compose" --env-file "$home/.env" -f "$home/compose/docker-compose.yml" -f "$home/compose/docker-compose.override.yml" "$@"
  else
    docker compose --project-directory "$home/compose" --env-file "$home/.env" -f "$home/compose/docker-compose.yml" "$@"
  fi
}}
"#,
        home = shell_quote(home)
    )
}
