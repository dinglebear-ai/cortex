#!/usr/bin/env bash
# Compatibility adapter. The canonical bootstrap owns acquisition and verification.
set -euo pipefail
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<'USAGE'
Install cortex from GitHub Releases.

Environment:
  INSTALL_DIR Destination directory (default: ~/.local/bin)
  CORTEX_RMCP_VERSION Release tag such as v3.8.0 (default: latest)
  CORTEX_RMCP_REPO GitHub repo owner/name (default: dinglebear-ai/cortex)
  CORTEX_INSTALL_SKIP_SETUP Defaults to 1 for this binary-only compatibility adapter.
Use the root install.sh for guided setup.
USAGE
  exit 0
fi
export CORTEX_INSTALL_BIN_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
export CORTEX_INSTALL_REPO="${CORTEX_INSTALL_REPO:-${CORTEX_RMCP_REPO:-dinglebear-ai/cortex}}"
export CORTEX_VERSION="${CORTEX_VERSION:-${CORTEX_RMCP_VERSION:-latest}}"
export CORTEX_INSTALL_RELEASE_BASE_URL="${CORTEX_INSTALL_RELEASE_BASE_URL:-${CORTEX_RMCP_RELEASE_BASE_URL:-}}"
export CORTEX_INSTALL_SKIP_SETUP="${CORTEX_INSTALL_SKIP_SETUP:-1}"
source_path="${BASH_SOURCE[0]:-}"
if [[ -n "$source_path" ]]; then
  script_dir="$(cd -- "$(dirname -- "$source_path")" && pwd)"
  if [[ -f "$script_dir/../install.sh" ]]; then
    exec sh "$script_dir/../install.sh" "$@"
  fi
fi
# Keep curl-pipe installation usable without assuming a repository checkout.
command -v curl >/dev/null || { printf 'curl is required\n' >&2; exit 1; }
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
ref="$CORTEX_VERSION"
if [[ "$ref" == latest ]]; then
  ref=main
elif [[ "$ref" != v* ]]; then
  ref="v$ref"
fi
curl -fsSL "https://raw.githubusercontent.com/$CORTEX_INSTALL_REPO/$ref/install.sh" -o "$tmpdir/install.sh"
sh "$tmpdir/install.sh" "$@"
