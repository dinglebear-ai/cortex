#!/usr/bin/env bash
set -euo pipefail

REPO="${CORTEX_RMCP_REPO:-dinglebear-ai/cortex}"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
VERSION="${CORTEX_RMCP_VERSION:-latest}"
RELEASE_BASE_URL="${CORTEX_RMCP_RELEASE_BASE_URL:-}"
BINARY_NAME="cortex"

usage() {
  cat <<'USAGE'
Install cortex from GitHub Releases.

Environment:
  INSTALL_DIR Destination directory (default: ~/.local/bin)
  CORTEX_RMCP_VERSION Release tag such as v3.8.0 (default: latest)
  CORTEX_RMCP_REPO GitHub repo owner/name (default: dinglebear-ai/cortex)
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then usage; exit 0; fi
need() { command -v "$1" >/dev/null 2>&1 || { printf 'error: %s is required\n' "$1" >&2; exit 1; }; }

target_asset() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    linux:x86_64|linux:amd64) printf 'cortex-linux-x86_64.tar.gz' ;;
    linux:aarch64|linux:arm64) printf 'cortex-linux-aarch64.tar.gz' ;;
    darwin:arm64) printf 'cortex-macos-arm64' ;;
    mingw*:x86_64|msys*:x86_64|cygwin*:x86_64) printf 'cortex-windows-x86_64.zip' ;;
    *) printf 'error: unsupported platform %s/%s\n' "$os" "$arch" >&2; exit 1 ;;
  esac
}

need curl; need install; need mktemp; need tar
asset="$(target_asset)"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
if [[ -n "$RELEASE_BASE_URL" ]]; then
  url="${RELEASE_BASE_URL%/}/${VERSION}/${asset}"
elif [[ "$VERSION" == "latest" ]]; then
  url="https://github.com/${REPO}/releases/latest/download/${asset}"
else
  url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"
fi

mkdir -p "$INSTALL_DIR"
if [[ ! -w "$INSTALL_DIR" ]]; then printf 'error: install dir is not writable: %s\n' "$INSTALL_DIR" >&2; exit 1; fi
printf 'Downloading %s\n' "$url" >&2
curl -fsSL "$url" -o "$tmpdir/$asset"
curl -fsSL "$url.sha256" -o "$tmpdir/$asset.sha256"
expected="$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset {print $1; exit}' "$tmpdir/$asset.sha256")"
if [[ ! "$expected" =~ ^[a-fA-F0-9]{64}$ ]]; then printf 'error: invalid checksum for %s\n' "$asset" >&2; exit 1; fi
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmpdir/$asset" | cut -d' ' -f1)"
else
  need shasum
  actual="$(shasum -a 256 "$tmpdir/$asset" | cut -d' ' -f1)"
fi
if [[ "$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')" != "$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')" ]]; then
  printf 'error: checksum mismatch for %s\n' "$asset" >&2
  exit 1
fi

case "$asset" in
  *.zip) need unzip; unzip -q "$tmpdir/$asset" -d "$tmpdir" ;;
  *.tar.gz) tar -xzf "$tmpdir/$asset" -C "$tmpdir" ;;
  cortex-macos-arm64) cp "$tmpdir/$asset" "$tmpdir/$BINARY_NAME" ;;
esac
binary="$tmpdir/$BINARY_NAME"
if [[ ! -f "$binary" && -f "$tmpdir/$BINARY_NAME.exe" ]]; then binary="$tmpdir/$BINARY_NAME.exe"; fi
if [[ ! -f "$binary" ]]; then printf 'error: archive did not contain %s binary\n' "$BINARY_NAME" >&2; exit 1; fi
install -m 755 "$binary" "$INSTALL_DIR/$BINARY_NAME"
printf 'Installed %s to %s/%s\n' "$BINARY_NAME" "$INSTALL_DIR" "$BINARY_NAME"
printf 'Run: %s --version\n' "$BINARY_NAME"
