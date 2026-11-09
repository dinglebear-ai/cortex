#!/usr/bin/env bash

surface_cli_resource_register() {
  local project="$1" compose_file="$2" parent_key="${3-topology}"
  SURFACE_RESOURCE_KEY="${4-surface-cli-topology}"
  SURFACE_RESOURCE_DIGEST="$(live_sha256 "$compose_file")"
  SURFACE_RESOURCE_LABELS="$(jq -cn --arg project "$project" '{"com.docker.compose.project":$project}')"
  SURFACE_RESOURCE_CLEANUP="$(jq -cn --arg file "$compose_file" --arg project "$project" \
    '["docker","compose","-f",$file,"-p",$project,"down","-v","--remove-orphans"]')"
  SURFACE_RESOURCE_VERIFY="$(jq -cn --arg project "$project" \
    '["sh","-ceu","test -z \"$(docker ps -aq --filter label=com.docker.compose.project=\"$1\")\"; test -z \"$(docker volume ls -q --filter label=com.docker.compose.project=\"$1\")\"; test -z \"$(docker network ls -q --filter label=com.docker.compose.project=\"$1\")\"","_",$project]')"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project PLANNED "$LIVE_RESOURCE_PROVIDER" "" '[]' "" "$SURFACE_RESOURCE_LABELS" '[]' "$parent_key"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project CREATING "$LIVE_RESOURCE_PROVIDER" "$project" '[]' "$SURFACE_RESOURCE_DIGEST" "$SURFACE_RESOURCE_LABELS" '[]' "$parent_key"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project IDENTIFIED "$LIVE_RESOURCE_PROVIDER" "$project" "$SURFACE_RESOURCE_CLEANUP" "$SURFACE_RESOURCE_DIGEST" "$SURFACE_RESOURCE_LABELS" "$SURFACE_RESOURCE_VERIFY" "$parent_key"
}

surface_cli_resource_created() {
  local project="$1" parent_key="${2-topology}"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project CREATED "$LIVE_RESOURCE_PROVIDER" "$project" "$SURFACE_RESOURCE_CLEANUP" "$SURFACE_RESOURCE_DIGEST" "$SURFACE_RESOURCE_LABELS" "$SURFACE_RESOURCE_VERIFY" "$parent_key"
}

surface_cli_resource_verified_removed() {
  local project="$1" parent_key="${2-topology}"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project CLEANING "$LIVE_RESOURCE_PROVIDER" "$project" "$SURFACE_RESOURCE_CLEANUP" "$SURFACE_RESOURCE_DIGEST" "$SURFACE_RESOURCE_LABELS" "$SURFACE_RESOURCE_VERIFY" "$parent_key"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project REMOVED "$LIVE_RESOURCE_PROVIDER" "$project" "$SURFACE_RESOURCE_CLEANUP" "$SURFACE_RESOURCE_DIGEST" "$SURFACE_RESOURCE_LABELS" "$SURFACE_RESOURCE_VERIFY" "$parent_key"
  live_resource_transition "$SURFACE_RESOURCE_KEY" compose-project VERIFIED "$LIVE_RESOURCE_PROVIDER" "$project" "$SURFACE_RESOURCE_CLEANUP" "$SURFACE_RESOURCE_DIGEST" "$SURFACE_RESOURCE_LABELS" "$SURFACE_RESOURCE_VERIFY" "$parent_key"
}

surface_cli_runtime_cleanup() {
  # The two synthetic HOME trees have finished their CLI cases. Their service
  # resources must be verified gone before deleting the generated credentials.
  [[ "${LIVE_RUN_HOME:?}" == "${LIVE_RUN_ROOT:?}/home" && "${LIVE_RUN_TMP:?}" == "$LIVE_RUN_ROOT/tmp" ]] || {
    live_die 'surface runtime cleanup refuses a HOME outside the exact run-owned fixture'; return 1;
  }
  python3 - "$LIVE_RUN_ROOT" "$LIVE_RUN_HOME" "$LIVE_RUN_TMP" <<'PYGUARD' || return 1
from pathlib import Path
import sys
root, home, temporary = map(Path, sys.argv[1:])
if (home.is_symlink() or temporary.is_symlink()
        or home.resolve() != root.resolve() / "home"
        or temporary.resolve() != root.resolve() / "tmp"):
    raise SystemExit("surface runtime cleanup refuses symlinked or redirected fixture paths")
PYGUARD
  jq -es 'group_by(.key) | map(last) |
    all(.[] | select(.key=="surface-cli-topology" or .key=="surface-backup-topology"); .state=="VERIFIED") and
    ([.[] | select(.key=="surface-cli-topology" or .key=="surface-backup-topology")]|length)==2' \
    "$LIVE_RUN_ROOT/resources.jsonl" >/dev/null || {
      live_die 'surface runtime cleanup requires both target teardowns to be verified'; return 1;
    }
  rm -rf -- "$LIVE_RUN_HOME"
  rm -f -- "$LIVE_RUN_TMP/cli-backup.json"
}
