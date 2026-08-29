#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d)"
trap 'rm -rf "$test_root"' EXIT

assert_mode_700() {
  local path="$1"
  [[ -d "$path" ]]
  [[ "$(stat -c '%a' "$path" 2>/dev/null || stat -f '%Lp' "$path")" == "700" ]]
}

default_home="$test_root/default-home"
mkdir -p "$default_home"
HOME="$default_home" CORTEX_ENV_FILE="$repo_dir/.env.example" \
  bash "$repo_dir/scripts/prepare-compose-dirs.sh" -f "$repo_dir/docker-compose.yml"
assert_mode_700 "$default_home/.cortex/backups"

custom_dir="$test_root/custom/backups"
HOME="$default_home" CORTEX_BACKUP_DIR="$custom_dir" \
  CORTEX_ENV_FILE="$repo_dir/.env.example" \
  bash "$repo_dir/scripts/prepare-compose-dirs.sh" -f "$repo_dir/docker-compose.yml"
assert_mode_700 "$custom_dir"

echo "Compose backup-directory provisioning contract passed"
