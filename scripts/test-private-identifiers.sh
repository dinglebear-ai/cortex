#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scanner="$repo_root/scripts/check-private-identifiers.sh"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT

expect_pass() {
  local name="$1" content="$2" file
  file="$fixture_dir/$name"
  printf '%s\n' "$content" > "$file"
  "$scanner" "$file" >/dev/null
}

expect_fail() {
  local name="$1" content="$2" file
  file="$fixture_dir/$name"
  printf '%s\n' "$content" > "$file"
  if "$scanner" "$file" >/dev/null 2>&1; then
    echo "expected private-identifier rejection: $name" >&2
    exit 1
  fi
}

expect_pass allowed-aurora.txt 'registry = https://aurora.tootie.tv/r/'
expect_fail agent-memory.md "private host: doo""kie"
expect_fail active-config.toml 'apprise_url = "http://198.51.100.2:8766"'
expect_fail mixed-allowlist.txt "https://aurora.too""tie.tv/r/ routes through too""tie"

echo '[private-identifiers-test] OK - negative fixtures reject scanner blind spots'
