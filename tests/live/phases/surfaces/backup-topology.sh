#!/usr/bin/env bash
# Dedicated managed recovery target, serialized before the ordinary CLI target.
# Resource ownership and interruption cleanup belong to the shared live ledger.
surface_backup_topology_run() {
  local binary="$1" image="$2" home project compose container port status=0
  home="$LIVE_RUN_ROOT/backup-home/.cortex"
  project="${LIVE_COMPOSE_PROJECT}-surface-backup"
  compose="$home/compose/docker-compose.yml"
  live_timeout_process_tree 10 docker info >/dev/null
  [[ -z "$(live_timeout_process_tree 10 docker ps -aq --filter 'name=^/cortex$')" ]] || {
    live_die 'managed backup requires the fixed cortex container name to be unoccupied'; return 1;
  }
  mkdir -p "$home/compose" "$home/data" "$home/backups" "$LIVE_RUN_TMP/backup-bin"
  chmod 700 "$home" "$home/compose"
  chmod 0777 "$home/data" "$home/backups"
  ln -s "$LIVE_CLI_FIXTURE_BIN/docker-real-wrapper" "$LIVE_RUN_TMP/backup-bin/docker"
  ln -s "$LIVE_CLI_FIXTURE_BIN/crontab" "$LIVE_RUN_TMP/backup-bin/crontab"
  # Both the lifecycle and its authenticated verification read this managed env.
  printf '%s\n' "CORTEX_TOKEN=$LIVE_CORTEX_TOKEN" "CORTEX_API_TOKEN=$LIVE_API_TOKEN" \
    'CORTEX_DB_PATH=/data/cortex.db' "CORTEX_DATA_VOLUME=$home/data" \
    'CORTEX_BACKUP_MIN_FREE_MB=32' \
    "CORTEX_API_ADMIN_TOKEN=$LIVE_ADMIN_TOKEN" "CORTEX_CURSOR_SIGNING_KEY=$LIVE_CURSOR_SIGNING_KEY" \
    "CORTEX_SERVER_ID=$LIVE_SERVER_INSTANCE_ID" >"$home/.env"
  chmod 600 "$home/.env"
  port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
  python3 - "$compose" "$home" "$image" "$port" "$project" <<'PY'
import json, os, sys
compose, home, image, port, project = sys.argv[1:]
environment = {"CORTEX_HOST": "0.0.0.0", "CORTEX_DB_PATH": "/data/cortex.db", "CORTEX_PORT": "3100"}
service = {"image": image, "pull_policy": "never", "container_name": "cortex",
           "command": ["cortex", "serve", "mcp"], "environment": environment, "env_file": [f"{home}/.env"],
           "volumes": [f"{home}/data:/data", f"{home}/backups:/backups", f"{home}:/cortex-home"],
           "ports": [f"127.0.0.1:{port}:3100/tcp"], "labels": {"cortex.live.run_id": os.environ["LIVE_RUN_ID"]}}
with open(compose, "w") as output:
    json.dump({"name": project, "services": {"cortex": service}}, output)
PY
  chmod 600 "$compose"
  surface_cli_resource_register "$project" "$compose" topology surface-backup-topology
  live_timeout_process_tree 75 docker compose -f "$compose" -p "$project" up -d --wait --wait-timeout 60
  surface_cli_resource_created "$project"
  container="$(live_timeout_process_tree 10 docker compose -f "$compose" -p "$project" ps -q cortex)"
  [[ -n "$container" ]] || { live_die 'managed backup container is missing'; return 1; }
  [[ "$(live_timeout_process_tree 10 docker inspect -f '{{ index .Config.Labels "com.docker.compose.project" }}' "$container")" == "$project" ]]
  [[ "$(live_timeout_process_tree 10 docker inspect -f '{{ index .Config.Labels "com.docker.compose.project.config_files" }}' "$container")" == "$compose" ]]
  port="$(live_timeout_process_tree 10 docker compose -f "$compose" -p "$project" port cortex 3100)"; port="${port##*:}"
  [[ "$port" =~ ^[0-9]+$ ]] || { live_die 'managed backup loopback port is missing'; return 1; }
  printf 'CORTEX_PORT=%s\n' "$port" >>"$home/.env"
  # A seed must exist before snapshotting. The runner writes and checks a real
  # SQLite marker, and recovery itself performs copied-DB integrity checks.
  export LIVE_BACKUP_HOME="$home" LIVE_BACKUP_PORT="$port"
  export LIVE_BACKUP_CLI_PATH="$LIVE_RUN_TMP/backup-bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
  python3 "$LIVE_PROJECT_ROOT/tests/live/phases/surfaces/backup_sweep.py" "$LIVE_SURFACE_CONTRACT" "$binary" "$LIVE_RUN_TMP/cli-backup.json" || status=$?
  # Recheck ownership immediately before the normal task-scoped teardown.
  container="$(live_timeout_process_tree 10 docker compose -f "$compose" -p "$project" ps -q cortex)"
  [[ -n "$container" ]]
  [[ "$(live_timeout_process_tree 10 docker inspect -f '{{ index .Config.Labels "com.docker.compose.project" }}' "$container")" == "$project" ]]
  [[ "$(live_timeout_process_tree 10 docker inspect -f '{{ index .Config.Labels "cortex.live.run_id" }}' "$container")" == "$LIVE_RUN_ID" ]]
  # These task-created bundles contain copies of the disposable credentials.
  # Delete them inside the owned container to preserve normal root/0700 backup
  # permissions, then leave the immutable Compose file available to the ledger.
  live_timeout_process_tree 30 docker exec --user 0:0 "$container" sh -c '
    find /backups -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
    find /cortex-home -mindepth 1 -maxdepth 1 -type d -name ".before-restore-*" -exec rm -rf -- {} +
  '
  live_timeout_process_tree 60 docker compose -f "$compose" -p "$project" down -v --remove-orphans
  [[ -z "$(live_timeout_process_tree 10 docker ps -aq --filter "label=com.docker.compose.project=$project")" ]]
  [[ -z "$(live_timeout_process_tree 10 docker ps -aq --filter 'name=^/cortex$')" ]] || {
    live_die 'managed backup teardown did not release the fixed cortex container name'; return 1;
  }
  surface_cli_resource_verified_removed "$project"
  : >"$home/.env"
  export LIVE_BACKUP_RESULTS="$LIVE_RUN_TMP/cli-backup.json"
  return "$status"
}
