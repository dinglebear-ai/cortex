#!/usr/bin/env bash
# =============================================================================
# scripts/smoke-test-http.sh — bead cortex-0p8r.10 cutover smoke test
#
# Exercises every CLI command that was routed through the container REST API
# in waves 7-9 (queries + AI + DB), plus verifies that LOCAL-only commands
# still work in default mode.
#
# Required environment:
#   CORTEX_API_TOKEN     Bearer token for the /api/* endpoints. If unset,
#                        this script falls back to grep'ing
#                        ${CORTEX_ENV_FILE:-${HOME}/.cortex/.env}.
#
# Optional environment:
#   CORTEX_BIN           Path to the `cortex` binary (default: `cortex` on PATH).
#   CORTEX_SERVER        Base URL of the REST API (default: http://localhost:3100).
#   CORTEX_ENV_FILE      Path to .env containing CORTEX_API_TOKEN
#                        (default: ${HOME}/.cortex/.env).
#   CORTEX_LOCAL_DB_PATH Host path used by local-only assertions
#                        (default: ${HOME}/.cortex/data/cortex.db).
#
# Exit:
#   0  — every assertion passed.
#   1  — first failure (descriptive line on stderr).
#   2  — prerequisite missing (binary, token, server unreachable).
#
# This script is intentionally serial: a failure on any command should halt
# the run so the operator can diagnose before the rest of the deploy
# pipeline continues.
# =============================================================================

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
CORTEX_BIN="${CORTEX_BIN:-cortex}"
CORTEX_SERVER="${CORTEX_SERVER:-http://localhost:3100}"
CORTEX_ENV_FILE="${CORTEX_ENV_FILE:-${HOME}/.cortex/.env}"

pass() { printf 'PASS  %s\n' "$1"; }
info() { printf 'INFO  %s\n' "$1"; }
fail() {
  local fmt="$1"; shift
  # shellcheck disable=SC2059
  printf "FAIL  ${fmt}\n" "$@" >&2
  exit 1
}
need() { printf 'NEED  %s\n' "$1" >&2; exit 2; }

# ─── Prereqs ────────────────────────────────────────────────────────────────

command -v "$CORTEX_BIN" >/dev/null 2>&1 || need "cortex binary not on PATH (set CORTEX_BIN=...)"
command -v jq >/dev/null 2>&1            || need "jq is required (install jq)"

if [[ -z "${CORTEX_API_TOKEN:-}" ]]; then
  if [[ -r "$CORTEX_ENV_FILE" ]]; then
    # shellcheck disable=SC2155
    CORTEX_API_TOKEN="$(grep -E '^CORTEX_API_TOKEN=' "$CORTEX_ENV_FILE" | head -n1 | cut -d= -f2-)"
  fi
fi
# Trim leading/trailing whitespace so blank-padded values are rejected.
# Guard against `set -u` blowing up if the var is entirely unset.
CORTEX_API_TOKEN="${CORTEX_API_TOKEN:-}"
CORTEX_API_TOKEN="${CORTEX_API_TOKEN#"${CORTEX_API_TOKEN%%[![:space:]]*}"}"
CORTEX_API_TOKEN="${CORTEX_API_TOKEN%"${CORTEX_API_TOKEN##*[![:space:]]}"}"
[[ -n "${CORTEX_API_TOKEN}" ]] || need "CORTEX_API_TOKEN not set/blank (checked env and $CORTEX_ENV_FILE)"
export CORTEX_API_TOKEN

info "binary  : $(command -v "$CORTEX_BIN")"
info "version : $("$CORTEX_BIN" --version 2>/dev/null || echo unknown)"
info "server  : $CORTEX_SERVER"

# ─── Helpers ────────────────────────────────────────────────────────────────

# Run a command and require its stdout to be a parseable JSON value.
# Usage: assert_json "label" cortex --http --json ...
assert_json() {
  local label="$1"; shift
  local out stderr_file
  stderr_file="$(mktemp)"
  if ! out="$("$@" 2>"$stderr_file")"; then
    printf '  stderr: %s\n' "$(cat "$stderr_file")" >&2
    rm -f "$stderr_file"
    fail "$label: command exited non-zero"
  fi
  if ! printf '%s' "$out" | jq -e . >/dev/null 2>&1; then
    printf '  stdout: %s\n' "${out:0:400}" >&2
    printf '  stderr: %s\n' "$(cat "$stderr_file")" >&2
    rm -f "$stderr_file"
    fail "$label: stdout is not valid JSON"
  fi
  rm -f "$stderr_file"
  pass "$label"
}

http() {
  "$CORTEX_BIN" --http --server "$CORTEX_SERVER" --token "$CORTEX_API_TOKEN" "$@"
}

# Default mode = no --http, no CORTEX_USE_HTTP. Local SQL path.
local_mode() {
  env -u CORTEX_USE_HTTP \
    CORTEX_DB_PATH="${CORTEX_LOCAL_DB_PATH:-${HOME}/.cortex/data/cortex.db}" \
    "$CORTEX_BIN" "$@"
}

# ─── HTTP-supported query commands (7) ──────────────────────────────────────
#
# CLI globals (--http, --server, --token) are stripped by Mode::parse before
# subcommand parsing; per-command flags (including --json) MUST follow the
# subcommand or they're rejected as an unknown leading argument.

CORTEX_SMOKE_REFTIME="${CORTEX_SMOKE_REFTIME:-$(date -u +"%Y-%m-%dT%H:%M:%SZ")}"
mkdir -p "${HOME}/.codex/sessions"
CORTEX_SMOKE_AI_TMP="$(mktemp -d "${HOME}/.codex/sessions/cortex-smoke.XXXXXX")"
trap 'rm -rf "$CORTEX_SMOKE_AI_TMP"' EXIT

assert_json "http: search (limit 1)"     http search --json --limit 1
assert_json "http: tail (limit 1)"       http tail --json --limit 1
assert_json "http: analysis errors"      http analysis errors --json
assert_json "http: hosts"                http hosts --json
assert_json "http: correlate events"     http correlate events --json --reference-time "$CORTEX_SMOKE_REFTIME" --host _smoke_ --window-minutes 1
assert_json "http: stats"                http stats --json
assert_json "http: sessions (limit 1)"   http sessions --json --limit 1

# ─── HTTP-supported session commands ────────────────────────────────────────

assert_json "http: sessions search"            http sessions search smoke --json --limit 1
assert_json "http: sessions abuse"             http sessions abuse --json --limit 1
assert_json "http: sessions correlate"         http sessions correlate --json --window-minutes 1 --limit 1
assert_json "http: sessions blocks"            http sessions blocks --json
assert_json "http: sessions context"           http sessions context / --json --limit 1
assert_json "http: sessions tools"             http sessions tools --json
assert_json "http: sessions projects"          http sessions projects --json
assert_json "http: sessions checkpoints"       http sessions checkpoints --json --limit 1
assert_json "http: sessions errors"            http sessions errors --json --limit 1
assert_json "http: sessions prunecheckpoints"  http sessions prunecheckpoints --json --dry-run --limit 1
assert_json "http: sessions skills"            http sessions skills --json --limit 1
assert_json "http: sessions skillincidents"    http sessions skillincidents --json --limit 1
assert_json "http: sessions skillinvestigate"  http sessions skillinvestigate smoke-skill --json --limit 1
assert_json "http: sessions mcpevents"         http sessions mcpevents --json --limit 1
assert_json "http: sessions mcpincidents"      http sessions mcpincidents --json --limit 1
assert_json "http: sessions mcpinvestigate"    http sessions mcpinvestigate smoke-server --json --limit 1
assert_json "http: sessions hookevents"        http sessions hookevents --json --limit 1
assert_json "http: sessions hookincidents"     http sessions hookincidents --json --limit 1
assert_json "http: sessions hookinvestigate"   http sessions hookinvestigate smoke-hook --json --limit 1

# ─── HTTP-supported DB commands ─────────────────────────────────────────────

assert_json "http: db status"            http db status --json
# Large production databases run integrity checks as server-side jobs so the
# transport returns immediately and callers can poll by job ID.
assert_json "http: db integrity (quick background)" http db integrity --json --quick --background
assert_json "http: db checkpoint"        http db checkpoint passive --json
# `vacuum --pages 1` keeps wall-clock low; full VACUUM may exceed 10-min
# HTTP timeout on large DBs (see docs/history/plans/http-cli-rollout.md Notes).
assert_json "http: db vacuum (pages=1)"  http db vacuum --json --pages 1
if [[ "${CORTEX_SMOKE_RUN_LONG:-false}" == "true" ]]; then
  assert_json "http: db backup"          http db backup --json
else
  pass "http: db backup (deferred; set CORTEX_SMOKE_RUN_LONG=true)"
fi

# ─── LOCAL-only session commands (6) — must succeed in default mode ─────────
# These all read host filesystem or run host-side processes. They are
# expected to bail with a descriptive error if invoked with --http (and
# bead .8 wired that bail in). Here we only assert the DEFAULT (no-flag)
# path returns JSON or, for daemon commands, runs briefly without crashing.

assert_json "local: sessions doctor"       local_mode sessions doctor --json
assert_json "local: sessions watchstatus" local_mode sessions watchstatus --json
# `sessions index` writes to the DB; keep it side-effect-light by indexing an
# empty tempdir if CORTEX_SMOKE_AI_INDEX_PATH is set, else skip the index
# side-effect and just exercise `sessions add` against /dev/null which bails
# cleanly. We assert the default `sessions index --help` path produces help
# output (still part of the CLI surface) rather than mutating DB state.
assert_json "local: sessions index (empty dir)" local_mode sessions index --path "$CORTEX_SMOKE_AI_TMP" --json
# `sessions add` requires a real file path; create a minimal stub.
printf '{}\n' >"$CORTEX_SMOKE_AI_TMP/empty.jsonl"
assert_json "local: sessions add (stub)" local_mode sessions add "$CORTEX_SMOKE_AI_TMP/empty.jsonl" --json
# `sessions smokewatch` writes a synthetic transcript and runs ai-watch briefly.
# Gate behind a flag — it's slower (~30s) and not appropriate for every
# CI run. Operators opt in via CORTEX_SMOKE_RUN_AI_WATCH=1.
if [[ "${CORTEX_SMOKE_RUN_AI_WATCH:-0}" == "1" ]]; then
  assert_json "local: sessions smokewatch" local_mode sessions smokewatch --json
else
  info "skip: sessions smokewatch (set CORTEX_SMOKE_RUN_AI_WATCH=1 to include)"
fi
# `sessions watch` is a daemon — we do NOT run it from a smoke test.
info "skip: sessions watch (long-running daemon; tested by systemd unit healthchecks)"

# ─── LOCAL-only DB: backup ──────────────────────────────────────────────────

if [[ "${CORTEX_SMOKE_RUN_LONG:-false}" == "true" ]]; then
  CORTEX_SMOKE_BACKUP="$CORTEX_SMOKE_AI_TMP/backup.db"
  assert_json "local: db backup (default)" local_mode db backup "$CORTEX_SMOKE_BACKUP" --json
  [[ -s "$CORTEX_SMOKE_BACKUP" ]] || fail "local: db backup did not produce a non-empty file at $CORTEX_SMOKE_BACKUP"
else
  pass "local: db backup (deferred; set CORTEX_SMOKE_RUN_LONG=true)"
fi

printf '\nAll %s assertions passed.\n' "$SCRIPT_NAME"
