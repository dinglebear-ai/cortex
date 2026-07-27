#!/usr/bin/env sh
# install.sh — thin bootstrap: acquire the cortex binary, then hand off to `cortex setup`.
# All prerequisite checks (Docker, ports, data dir) happen inside `cortex setup`.
set -eu

REPO="${CORTEX_INSTALL_REPO:-dinglebear-ai/cortex}"
VERSION="${CORTEX_VERSION:-latest}"
PREFIX="${CORTEX_INSTALL_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
BIN="$BIN_DIR/cortex"
DRY_RUN="${CORTEX_INSTALL_DRY_RUN:-0}"
SKIP_SETUP="${CORTEX_INSTALL_SKIP_SETUP:-0}"
# METHOD: pull (download GitHub release tarball) or build (cargo build --release).
METHOD="${CORTEX_INSTALL_METHOD:-pull}"

say() {
  printf '%s\n' "$*" >&2
}

fail() {
  say "cortex install: $*"
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

# Asset naming MUST match .github/workflows/release.yml, which packages the
# linux build as `cortex-linux-x86_64.tar.gz` (+ `.sha256`).
detect_target() {
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os:$arch" in
    linux:x86_64|linux:amd64) printf 'linux-x86_64' ;;
    mingw*:*|msys*:*|cygwin*:*)
      fail "Windows detected — use install.ps1 instead: irm https://raw.githubusercontent.com/dinglebear-ai/cortex/main/install.ps1 | iex" ;;
    *) fail "unsupported platform $os/$arch" ;;
  esac
}

asset_url() {
  target="$1"
  if [ "$VERSION" = "latest" ]; then
    printf 'https://github.com/%s/releases/latest/download/cortex-%s.tar.gz' "$REPO" "$target"
  else
    printf 'https://github.com/%s/releases/download/%s/cortex-%s.tar.gz' "$REPO" "$VERSION" "$target"
  fi
}

check_prereqs() {
  need curl
  need sha256sum
  need install
  need tar
}

build_from_source() {
  need cargo
  say "Building cortex from source (cargo build --release)..."
  cargo build --release --bin cortex
  BUILT_PATH="$(pwd)/target/release/cortex"
  [ -f "$BUILT_PATH" ] || fail "cargo build succeeded but cortex binary not found at $BUILT_PATH"
  DOWNLOADED_PATH="$BUILT_PATH"
  DOWNLOAD_TMPDIR=""
  DOWNLOAD_TMPDIR_CREATED=0
}

download_and_verify() {
  target="$1"
  CREATED_TMPDIR=0
  if [ "${CORTEX_INSTALL_TMPDIR:-}" ]; then
    tmpdir="$CORTEX_INSTALL_TMPDIR"
    case "$tmpdir" in
      ""|"/") fail "unsafe CORTEX_INSTALL_TMPDIR: $tmpdir" ;;
    esac
    [ -d "$tmpdir" ] || fail "CORTEX_INSTALL_TMPDIR must be an existing directory"
    tmp_owner="$(ls -nd "$tmpdir" | awk '{print $3}')"
    [ "$tmp_owner" = "$(id -u)" ] || fail "CORTEX_INSTALL_TMPDIR must be owned by the current user"
  else
    tmpdir="$(mktemp -d)"
    CREATED_TMPDIR=1
  fi
  bin_url="${CORTEX_INSTALL_BIN_URL:-$(asset_url "$target")}"
  sha_url="${CORTEX_INSTALL_SHA256_URL:-$bin_url.sha256}"
  archive="$tmpdir/cortex.tar.gz"
  checksum="$tmpdir/cortex.tar.gz.sha256"

  say "Downloading $bin_url"
  curl -fsSL "$bin_url" -o "$archive"
  say "Downloading $sha_url"
  curl -fsSL "$sha_url" -o "$checksum"

  expected="$(awk '{print $1; exit}' "$checksum")"
  [ -n "$expected" ] || fail "checksum file is empty"
  actual="$(sha256sum "$archive" | awk '{print $1}')"
  [ "$expected" = "$actual" ] || fail "checksum mismatch for downloaded cortex archive"

  # Extract the `cortex` binary from the release tarball (release.yml tars a
  # single `cortex` member at the archive root).
  tar -xzf "$archive" -C "$tmpdir" cortex || fail "failed to extract cortex from archive"
  [ -f "$tmpdir/cortex" ] || fail "release archive did not contain a cortex binary"
  chmod +x "$tmpdir/cortex"
  DOWNLOADED_PATH="$tmpdir/cortex"
  DOWNLOAD_TMPDIR="$tmpdir"
  DOWNLOAD_TMPDIR_CREATED="$CREATED_TMPDIR"
}

cleanup_download() {
  if [ "${DOWNLOAD_TMPDIR_CREATED:-0}" = "1" ] && [ -n "${DOWNLOAD_TMPDIR:-}" ]; then
    rm -rf "$DOWNLOAD_TMPDIR"
  fi
}

main() {
  if [ "$DRY_RUN" = "1" ]; then
    target="$(detect_target)"
    say "Dry run OK: target=$target prefix=$PREFIX repo=$REPO version=$VERSION method=$METHOD"
    exit 0
  fi

  case "$METHOD" in
    pull)
      target="$(detect_target)"
      check_prereqs
      download_and_verify "$target"
      trap cleanup_download EXIT HUP INT TERM
      ;;
    build)
      build_from_source
      ;;
    *)
      fail "unknown METHOD '$METHOD'; expected pull or build"
      ;;
  esac

  mkdir -p "$BIN_DIR"
  install -m 0755 "$DOWNLOADED_PATH" "$BIN"
  say "Installed $BIN"

  if [ "$SKIP_SETUP" != "1" ]; then
    # Hand off to cortex setup. Prerequisite checks (data dir, port, env) run
    # inside `cortex setup repair`.
    "$BIN" setup repair
  fi
}

main "$@"
