use super::*;

pub(crate) fn command_phase<const N: usize>(name: &'static str, args: [&str; N]) -> SetupPhase {
    let timer = PhaseTimer::start(name);
    let program = if name == "docker compose" {
        "docker"
    } else {
        name
    };
    match crate::env::command(program).args(args).output() {
        Ok(output) if output.status.success() => timer.finish(
            SetupStatus::Ok,
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("available")
                .to_string(),
        ),
        Ok(output) => timer.finish(
            SetupStatus::Error,
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("command failed")
                .to_string(),
        ),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            timer.finish(SetupStatus::Error, "not found on PATH")
        }
        Err(err) => timer.finish(SetupStatus::Error, err.to_string()),
    }
}

pub(crate) fn ensure_network_phase(
    phases: &mut Vec<SetupPhase>,
    env: Option<&BTreeMap<String, String>>,
) {
    let timer = PhaseTimer::start("docker-network");
    let network = env
        .and_then(|env| env.get("DOCKER_NETWORK"))
        .map(String::as_str)
        .unwrap_or("cortex");
    let inspect = crate::env::command("docker")
        .args(["network", "inspect", network])
        .output();
    if inspect.as_ref().is_ok_and(|output| output.status.success()) {
        phases.push(timer.finish(SetupStatus::Ok, format!("{network} exists")));
        return;
    }
    match crate::env::command("docker")
        .args(["network", "create", network])
        .output()
    {
        Ok(output) if output.status.success() => {
            phases.push(timer.finish(SetupStatus::Ok, format!("created {network}")))
        }
        Ok(output) => phases.push(
            timer.finish(
                SetupStatus::Error,
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("docker network create failed"),
            ),
        ),
        Err(err) => phases.push(timer.finish(SetupStatus::Error, err.to_string())),
    }
}

pub(crate) fn run_compose_phase(compose_dir: &Path, env_path: &Path, args: &[&str]) -> SetupPhase {
    let timer = PhaseTimer::start(if args.first() == Some(&"pull") {
        "compose-pull"
    } else {
        "compose-up"
    });
    let mut command = crate::env::command("docker");
    command
        .arg("compose")
        .arg("--env-file")
        .arg(env_path)
        .arg("-f")
        .arg(compose_dir.join("docker-compose.yml"));
    let override_path = compose_dir.join("docker-compose.override.yml");
    if override_path.exists() {
        command.arg("-f").arg(override_path);
    }
    let recovery_path = compose_dir.join("docker-compose.recovery.yml");
    if recovery_path.exists() {
        command.arg("-f").arg(recovery_path);
    }
    command
        .args(args)
        .current_dir(compose_dir)
        .env("PWD", compose_dir);
    match command.output() {
        Ok(output) if output.status.success() => timer.finish(
            SetupStatus::Ok,
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .last()
                .unwrap_or("ok")
                .to_string(),
        ),
        Ok(output) => timer.finish(
            SetupStatus::Error,
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("docker compose failed")
                .to_string(),
        ),
        Err(err) => timer.finish(SetupStatus::Error, err.to_string()),
    }
}

#[cfg(test)]
pub(crate) fn health_phase(env: &Option<BTreeMap<String, String>>) -> SetupPhase {
    let timer = PhaseTimer::start("health");
    let port = env
        .as_ref()
        .and_then(|env| env.get("CORTEX_PORT"))
        .map(String::as_str)
        .unwrap_or("3100");
    let url = format!("http://127.0.0.1:{port}/health");
    match crate::env::command("curl")
        .args(["-fsS", "--max-time", "5", &url])
        .output()
    {
        Ok(output) if output.status.success() => {
            timer.finish(SetupStatus::Ok, format!("{url} ready"))
        }
        Ok(output) => timer.finish(
            SetupStatus::Error,
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("health check failed"),
        ),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            timer.finish(SetupStatus::Error, "curl not found; skipped health check")
        }
        Err(err) => timer.finish(SetupStatus::Error, err.to_string()),
    }
}
