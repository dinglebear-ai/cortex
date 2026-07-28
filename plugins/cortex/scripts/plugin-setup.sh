#!/usr/bin/env bash
# Manual setup adapter for cortex (not a Claude Code lifecycle hook).
# Adapts plugin settings to env and delegates setup to an installed cortex binary.
set -euo pipefail

: "${CLAUDE_PLUGIN_ROOT:=$(cd "$(dirname "$0")/.." && pwd)}"
: "${CLAUDE_PLUGIN_DATA:=${HOME}/.claude/plugins/data/cortex-jmagar-lab}"
: "${CORTEX_DATA_DIR:=${CLAUDE_PLUGIN_DATA}}"

reject_unsafe_value() {
  local name="$1" value="${2:-}"
  if [[ "${value}" == *$'\n'* || "${value}" == *$'\r'* ]]; then
    printf 'cortex plugin setup: %s must not contain newlines\n' "${name}" >&2
    exit 2
  fi
}

export_if_set() {
  local env_name="$1" option_name="$2" value
  value="$(printenv "${option_name}" || true)"
  reject_unsafe_value "${option_name}" "${value}"
  [[ -n "${value}" ]] || return 0
  export "${env_name}=${value}"
}

cortex_binary() {
  local binary="${CORTEX_MCP_BIN:-cortex}"
  if command -v "${binary}" >/dev/null 2>&1; then
    command -v "${binary}"
    return 0
  fi

  printf 'cortex plugin setup: cortex is not installed or not on PATH.\n' >&2
  printf 'Install cortex separately, then run: cortex setup\n' >&2
  return 1
}

validate_client() {
  local server_url
  server_url="$(strip_trailing_mcp_path "${CLAUDE_PLUGIN_OPTION_SERVER_URL:-http://localhost:3100}")"
  if curl -fsS --connect-timeout 2 --max-time 5 "${server_url%/}/health" >/dev/null 2>&1; then
    echo "cortex: connected to ${server_url%/}"
  else
    echo "WARNING: cortex server at ${server_url%/} is not reachable" >&2
  fi
}

strip_trailing_mcp_path() {
  local url="${1%/}"
  if [[ "${url}" == */mcp ]]; then
    url="${url%/mcp}"
  fi
  printf '%s\n' "${url}"
}

append_csv_unique() {
  local csv="$1" value="$2" item
  local -a items
  [[ -n "${value}" ]] || { printf '%s\n' "${csv}"; return; }
  local IFS=','
  read -r -a items <<< "${csv}"
  for item in "${items[@]}"; do
    item="${item#"${item%%[![:space:]]*}"}"
    item="${item%"${item##*[![:space:]]}"}"
    [[ "${item}" != "${value}" ]] || { printf '%s\n' "${csv}"; return; }
  done
  if [[ -n "${csv}" ]]; then
    printf '%s,%s\n' "${csv}" "${value}"
  else
    printf '%s\n' "${value}"
  fi
}

codex_oauth_callback_url() {
  local config="${HOME}/.codex/config.toml"
  [[ -f "${config}" ]] || return 0
  awk -F= '
    $1 ~ /^[[:space:]]*mcp_oauth_callback_url[[:space:]]*$/ {
      value = $2
      sub(/^[[:space:]]*"/, "", value)
      sub(/"[[:space:]]*$/, "", value)
      print value
      exit
    }
  ' "${config}"
}

prepare_oauth_env() {
  local auth_mode="${CORTEX_AUTH_MODE:-bearer}"
  local server_url="${CLAUDE_PLUGIN_OPTION_SERVER_URL:-http://localhost:3100}"
  local redirects="${CORTEX_AUTH_ALLOWED_REDIRECT_URIS:-}"
  local codex_callback

  [[ "${auth_mode}" == "oauth" ]] || return 0
  if [[ -z "${CORTEX_PUBLIC_URL:-}" && "${server_url}" == https://* ]]; then
    export CORTEX_PUBLIC_URL
    CORTEX_PUBLIC_URL="$(strip_trailing_mcp_path "${server_url}")"
  fi

  redirects="$(append_csv_unique "${redirects}" "https://claude.ai/api/mcp/auth_callback")"
  redirects="$(append_csv_unique "${redirects}" "https://claudeai.ai/api/mcp/auth_callback")"
  codex_callback="$(codex_oauth_callback_url)"
  if [[ -n "${codex_callback}" ]]; then
    redirects="$(append_csv_unique "${redirects}" "${codex_callback}")"
  fi
  export CORTEX_AUTH_ALLOWED_REDIRECT_URIS="${redirects}"
  export CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH="${CORTEX_AUTH_DISABLE_STATIC_TOKEN_WITH_OAUTH:-false}"
}

main() {
  local is_server="${CLAUDE_PLUGIN_OPTION_IS_SERVER:-true}"
  local cortex_bin

  reject_unsafe_value "CLAUDE_PLUGIN_OPTION_API_TOKEN" "${CLAUDE_PLUGIN_OPTION_API_TOKEN:-}"
  export_if_set CORTEX_TOKEN CLAUDE_PLUGIN_OPTION_API_TOKEN
  export_if_set CORTEX_SERVER_URL CLAUDE_PLUGIN_OPTION_SERVER_URL
  export_if_set CORTEX_AUTH_MODE CLAUDE_PLUGIN_OPTION_AUTH_MODE
  export_if_set CORTEX_PUBLIC_URL CLAUDE_PLUGIN_OPTION_PUBLIC_URL
  export_if_set CORTEX_GOOGLE_CLIENT_ID CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_ID
  export_if_set CORTEX_GOOGLE_CLIENT_SECRET CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_SECRET
  export_if_set CORTEX_AUTH_ADMIN_EMAIL CLAUDE_PLUGIN_OPTION_AUTH_ADMIN_EMAIL
  export_if_set CORTEX_AUTH_ALLOWED_REDIRECT_URIS CLAUDE_PLUGIN_OPTION_AUTH_ALLOWED_REDIRECT_URIS
  export_if_set CORTEX_RECEIVER_HOST CLAUDE_PLUGIN_OPTION_CORTEX_RECEIVER_HOST
  export_if_set CORTEX_RECEIVER_PORT CLAUDE_PLUGIN_OPTION_CORTEX_RECEIVER_PORT
  export_if_set CORTEX_RECEIVER_HOST_PORT CLAUDE_PLUGIN_OPTION_CORTEX_RECEIVER_HOST_PORT
  export_if_set CORTEX_HOST CLAUDE_PLUGIN_OPTION_MCP_HOST
  export_if_set CORTEX_PORT CLAUDE_PLUGIN_OPTION_MCP_PORT
  export_if_set CORTEX_MAX_DB_SIZE_MB CLAUDE_PLUGIN_OPTION_MAX_DB_SIZE_MB
  export_if_set CORTEX_DATA_VOLUME CLAUDE_PLUGIN_OPTION_DATA_DIR
  export_if_set CORTEX_RETENTION_DAYS CLAUDE_PLUGIN_OPTION_RETENTION_DAYS
  export_if_set CORTEX_BATCH_SIZE CLAUDE_PLUGIN_OPTION_BATCH_SIZE
  export_if_set CORTEX_WRITE_CHANNEL_CAPACITY CLAUDE_PLUGIN_OPTION_WRITE_CHANNEL_CAPACITY
  export_if_set CORTEX_DOCKER_INGEST_ENABLED CLAUDE_PLUGIN_OPTION_DOCKER_INGEST_ENABLED
  export_if_set CORTEX_DOCKER_HOSTS CLAUDE_PLUGIN_OPTION_FLEET_HOSTS
  export_if_set NO_AUTH CLAUDE_PLUGIN_OPTION_NO_AUTH
  prepare_oauth_env

  if [[ "${is_server}" != "true" ]]; then
    validate_client
    return 0
  fi

  mkdir -p "${CORTEX_DATA_DIR}"
  chmod 700 "${CORTEX_DATA_DIR}" 2>/dev/null || true
  export CORTEX_DATA_DIR

  if ! cortex_bin="$(cortex_binary)"; then
    return 0
  fi

  "${cortex_bin}" setup pluginhook "$@"
}

main "$@"
