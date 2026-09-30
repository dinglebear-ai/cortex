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

# Exercise Git enumeration too, not only the explicit-path fixture mode.
repo_fixture="$fixture_dir/repo"
mkdir -p "$repo_fixture/scripts" "$repo_fixture/docs/reference/mcp" "$repo_fixture/deploy"
cp "$scanner" "$repo_fixture/scripts/check-private-identifiers.sh"
: > "$repo_fixture/config.toml"
: > "$repo_fixture/docker-compose.yml"
mkdir "$repo_fixture/config"
: > "$repo_fixture/config/Dockerfile"
: > "$repo_fixture/docs/reference/mcp/deploy.md"
: > "$repo_fixture/deploy/README.md"
printf "# Shared instructions\n" > "$repo_fixture/AGENTS.md"
ln -s AGENTS.md "$repo_fixture/CLAUDE.md"
ln -s AGENTS.md "$repo_fixture/GEMINI.md"
git -C "$repo_fixture" init -q
git -C "$repo_fixture" add .
"$repo_fixture/scripts/check-private-identifiers.sh" >/dev/null
printf "%s\n" "private host: doo""kie" > "$repo_fixture/AGENTS.md"
if "$repo_fixture/scripts/check-private-identifiers.sh" >/dev/null 2>&1; then
  echo "expected canonical AGENTS.md to be scanned" >&2
  exit 1
fi
printf "# Shared instructions\n" > "$repo_fixture/AGENTS.md"
printf "%s\n" "private host: doo""kie" > "$fixture_dir/private.txt"
ln -s "$fixture_dir/private.txt" "$repo_fixture/linked.txt"
git -C "$repo_fixture" add linked.txt
if "$repo_fixture/scripts/check-private-identifiers.sh" >/dev/null 2>&1; then
  echo "expected unrelated tracked symlink to be scanned" >&2
  exit 1
fi

echo '[private-identifiers-test] OK - negative fixtures reject scanner blind spots'
