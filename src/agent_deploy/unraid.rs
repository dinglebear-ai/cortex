//! Transactional replacement of a containerized host agent.
use super::shell_quote;

pub(super) fn replacement_script(
    container: &str,
    env_path: &str,
    staged_env: &str,
    run: &str,
    version: &str,
) -> String {
    format!(
        r#"set -eu
command -v timeout >/dev/null || {{ echo 'Unraid deployment requires timeout for bounded Docker validation' >&2; exit 1; }}
container={container}
env_path={env_path}
staged={staged}
expected={version}
lock="$env_path.deploy-lock"
previous="$container.previous-$$"
backup="$env_path.rollback-$$"
had_env=0
had_container=0
was_running=false
renamed=0
replaced=0
committed=0
if ! mkdir "$lock"; then
  rm -f "$staged"
  echo 'agent deployment already in progress; inspect retained deployment lock' >&2
  exit 1
fi
finish() {{
  status=$?
  trap - EXIT HUP INT TERM
  set +e
  if [ "$committed" = 0 ]; then
    recovery_failed=0
    if [ "$replaced" = 1 ]; then
      if candidate=$(timeout 15 docker container ls -a --filter "name=^/$container$" --format '{{{{.Names}}}}'); then
        if [ "$candidate" = "$container" ]; then
          timeout 15 docker rm -f "$container" >/dev/null 2>&1 || recovery_failed=1
        fi
      else
        recovery_failed=1
      fi
      if [ "$had_env" = 1 ]; then
        cp -p "$backup" "$env_path" || recovery_failed=1
      else
        rm -f "$env_path" || recovery_failed=1
      fi
    fi
    if [ "$renamed" = 1 ]; then
      if saved=$(timeout 15 docker container ls -a --filter "name=^/$previous$" --format '{{{{.Names}}}}'); then
        if [ "$saved" = "$previous" ]; then
          timeout 15 docker rename "$previous" "$container" || recovery_failed=1
        elif ! timeout 15 docker inspect "$container" >/dev/null 2>&1; then
          recovery_failed=1
        fi
      else
        recovery_failed=1
      fi
      if [ "$was_running" = true ]; then
        timeout 15 docker start "$container" >/dev/null || recovery_failed=1
      fi
    fi
    if [ "$recovery_failed" = 1 ]; then
      echo "agent rollback failed; retained $previous and $backup for recovery" >&2
      exit 1
    fi
  else
    if [ "$had_container" = 1 ]; then
      timeout 15 docker rm -f "$previous" >/dev/null || echo "retained old container $previous" >&2
    fi
  fi
  rm -f "$staged" "$backup"
  rmdir "$lock"
  exit "$status"
}}
trap finish EXIT
trap 'exit 1' HUP INT TERM
if [ -f "$env_path" ]; then
  cp -p "$env_path" "$backup"
  had_env=1
fi
existing=$(timeout 15 docker container ls -a --filter "name=^/$container$" --format '{{{{.Names}}}}')
if [ "$existing" = "$container" ]; then
  had_container=1
  was_running=$(timeout 15 docker inspect --format '{{{{.State.Running}}}}' "$container")
  renamed=1
  timeout 15 docker rename "$container" "$previous"
  if [ "$was_running" = true ]; then timeout 15 docker stop "$previous" >/dev/null; fi
fi
# Mark the live environment as changed before attempting its replacement.
replaced=1
mv "$staged" "$env_path"
timeout 15 {run}
# Explicit process health: correct binary and sustained Running state.
# This is not a delivery receipt from the authoritative server.
reported=$(timeout 15 docker exec "$container" cortex --version)
[ "$reported" = "cortex $expected" ] || {{ echo 'agent version validation failed' >&2; exit 1; }}
attempt=0
while [ "$attempt" -lt 10 ]; do
  [ "$(timeout 15 docker inspect --format '{{{{.State.Running}}}}' "$container")" = true ] || {{ echo 'agent exited during health validation' >&2; exit 1; }}
  sleep 1
  attempt=$((attempt + 1))
done
[ "$(timeout 15 docker inspect --format '{{{{.State.Running}}}}' "$container")" = true ]
committed=1
printf '%s\n' 'agent version and sustained process health validated; delivery unverified'
"#,
        container = shell_quote(container),
        env_path = shell_quote(env_path),
        staged = shell_quote(staged_env),
        version = shell_quote(version),
    )
}

#[cfg(test)]
#[path = "unraid_tests.rs"]
mod tests;
