#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
renderer="$repo_root/scripts/render-deploy-templates.sh"
output_dir="$(mktemp -d)"
trap 'rm -rf "$output_dir"' EXIT

expect_rejected() {
  local value="$1"
  if CORTEX_OTLP_ENDPOINT="$value" "$renderer" "$output_dir" >/dev/null 2>&1; then
    echo "expected unsafe endpoint rejection" >&2
    exit 1
  fi
}

expect_rejected 'http://cortex.example.invalid:3100'
expect_rejected $'http://valid.example/line\nbreak'
expect_rejected 'http://valid.example/"quote'
expect_rejected 'http://valid.example/\backslash'

CORTEX_OTLP_ENDPOINT='https://cortex.example.com:3100' "$renderer" "$output_dir" >/dev/null
jq empty "$output_dir/claude-code-settings.example.json"
taplo check "$output_dir/codex-config.example.toml"
echo '[render-deploy-templates-test] OK - safe URL renders; hostile values fail closed'
