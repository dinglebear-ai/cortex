#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$script_dir/check-agent-memory-symlinks.sh"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
git -C "$fixture" init -q

make_instructions() {
  mkdir -p "$1"
  printf "# Canonical instructions\n" > "$1/AGENTS.md"
  ln -s AGENTS.md "$1/CLAUDE.md"
  ln -s AGENTS.md "$1/GEMINI.md"
}

checks=0
expect_pass() {
  if ! bash "$checker" "$fixture" > "$fixture/check.log" 2>&1; then
    printf "FAIL: expected valid instruction layout: %s\n" "$1" >&2
    exit 1
  fi
  checks=$((checks + 1))
}
expect_fail() {
  if bash "$checker" "$fixture" > "$fixture/check.log" 2>&1; then
    printf "FAIL: accepted invalid instruction layout: %s\n" "$1" >&2
    exit 1
  fi
  checks=$((checks + 1))
}

expect_fail "missing root instructions"
make_instructions "$fixture"
expect_pass "untracked canonical root"
git -C "$fixture" add AGENTS.md CLAUDE.md GEMINI.md
expect_pass "tracked canonical root"
make_instructions "$fixture/scoped directory"
expect_pass "new scoped directory with spaces"

rm "$fixture/CLAUDE.md"
printf "# Independent copy\n" > "$fixture/CLAUDE.md"
expect_fail "independent Claude copy"
rm "$fixture/CLAUDE.md"
ln -s missing.md "$fixture/CLAUDE.md"
expect_fail "wrong and broken target"
rm "$fixture/CLAUDE.md"
ln -s "$fixture/AGENTS.md" "$fixture/CLAUDE.md"
expect_fail "absolute target"
rm "$fixture/CLAUDE.md"
ln -s AGENTS.md "$fixture/CLAUDE.md"

rm "$fixture/GEMINI.md"
expect_fail "missing Gemini alias"
ln -s AGENTS.md "$fixture/GEMINI.md"
rm "$fixture/AGENTS.md"
expect_fail "broken canonical aliases"
printf "# Canonical instructions\n" > "$fixture/AGENTS.md"

rm "$fixture/CLAUDE.md" "$fixture/AGENTS.md"
printf "# Wrong owner\n" > "$fixture/CLAUDE.md"
ln -s CLAUDE.md "$fixture/AGENTS.md"
expect_fail "reversed canonical ownership"
rm "$fixture/AGENTS.md" "$fixture/CLAUDE.md"
printf "# Canonical instructions\n" > "$fixture/AGENTS.md"
ln -s AGENTS.md "$fixture/CLAUDE.md"

mkdir "$fixture/orphan"
printf "# Orphan\n" > "$fixture/orphan/CLAUDE.md"
expect_fail "orphan scoped alias"
rm -r "$fixture/orphan"
printf "generated/\n" > "$fixture/.gitignore"
mkdir "$fixture/generated"
printf "# Ignored dependency docs\n" > "$fixture/generated/CLAUDE.md"
expect_pass "ignored generated content"

# Local guidance is optional, but must never become shared repository content.
printf "Read and follow AGENTS.md before any work.\n" > "$fixture/AGENTS.override.md"
ln -s AGENTS.override.md "$fixture/CLAUDE.local.md"
expect_fail "local files not ignored"
printf "AGENTS.override.md\nCLAUDE.local.md\nCLAUDE.md.local\n" >> "$fixture/.gitignore"
expect_pass "ignored local source and alias"
rm "$fixture/CLAUDE.local.md"
printf "Independent local copy\n" > "$fixture/CLAUDE.local.md"
expect_fail "independent local Claude copy"
rm "$fixture/CLAUDE.local.md"
ln -s AGENTS.md "$fixture/CLAUDE.local.md"
expect_fail "local alias pointing at shared instructions"
rm "$fixture/CLAUDE.local.md"
ln -s AGENTS.override.md "$fixture/CLAUDE.local.md"
rm "$fixture/AGENTS.override.md"
expect_fail "missing local canonical file"
printf "Only local context\n" > "$fixture/AGENTS.override.md"
expect_fail "override omits shared instructions"
printf "Read and follow AGENTS.md before any work.\n" > "$fixture/AGENTS.override.md"
git -C "$fixture" add -f AGENTS.override.md
expect_fail "force-staged private override"
git -C "$fixture" rm --cached -q AGENTS.override.md
expect_pass "private override removed from index"

printf "[agent-memory-test] OK — %s regression cases\n" "$checks"
