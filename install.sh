#!/usr/bin/env sh
# install.sh — thin bootstrap: acquire the cortex binary, then hand off to `cortex setup`.
# All prerequisite checks (Docker, ports, data dir) happen inside `cortex setup`.
set -eu

REPO="${CORTEX_INSTALL_REPO:-dinglebear-ai/cortex}"
VERSION="${CORTEX_VERSION:-latest}"
PREFIX="${CORTEX_INSTALL_PREFIX:-$HOME/.local}"
BIN_DIR="${CORTEX_INSTALL_BIN_DIR:-$PREFIX/bin}"
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
    linux:aarch64|linux:arm64) printf 'linux-aarch64' ;;
    darwin:arm64|darwin:aarch64) printf 'macos-arm64' ;;
    mingw*:*|msys*:*|cygwin*:*)
      fail "Windows detected — use install.ps1 instead: irm https://raw.githubusercontent.com/dinglebear-ai/cortex/main/install.ps1 | iex" ;;
    *) fail "unsupported platform $os/$arch" ;;
  esac
}

asset_name() {
  case "$1" in
    macos-arm64) printf 'cortex-macos-arm64' ;;
    *) printf 'cortex-%s.tar.gz' "$1" ;;
  esac
}

asset_url() {
  asset="$(asset_name "$1")"
  version="$VERSION"
  case "$version" in latest|v*) ;; *) version="v$version" ;; esac
  if [ "${CORTEX_INSTALL_RELEASE_BASE_URL:-}" ]; then
    printf '%s/%s/%s' "${CORTEX_INSTALL_RELEASE_BASE_URL%/}" "$version" "$asset"
  elif [ "$version" = "latest" ]; then
    printf 'https://github.com/%s/releases/latest/download/%s' "$REPO" "$asset"
  else
    printf 'https://github.com/%s/releases/download/%s/%s' "$REPO" "$version" "$asset"
  fi
}

check_prereqs() {
  need curl
  if ! command -v sha256sum >/dev/null 2>&1; then need shasum; fi
  need install
  need tar
}

build_from_source() {
  need cargo
  say "Building cortex from source (cargo build --release --locked)..."
  # Cargo resolves target-dir from repository config, environment and CLI. Ask it
  # for the effective directory rather than assuming a conventional target/.
  need python3
  target_dir="$(cargo metadata --no-deps --format-version 1 --locked | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
  cargo build --release --locked --bin cortex
  BUILT_PATH="$target_dir/release/cortex"
  [ -f "$BUILT_PATH" ] || fail "cargo build succeeded but cortex binary not found at $BUILT_PATH"
  DOWNLOADED_PATH="$BUILT_PATH"
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
  DOWNLOAD_TMPDIR="$tmpdir"
  DOWNLOAD_TMPDIR_CREATED="$CREATED_TMPDIR"
  archive="$tmpdir/$(asset_name "$target")"
  checksum="$archive.sha256"

  say "Downloading $bin_url"
  curl -fsSL "$bin_url" -o "$archive"
  say "Downloading $sha_url"
  curl -fsSL "$sha_url" -o "$checksum"

  expected="$(awk '{print $1; exit}' "$checksum")"
  case "$expected" in *[!a-fA-F0-9]*|"") fail "invalid checksum" ;; esac
  [ "${#expected}" -eq 64 ] || fail "invalid checksum"
  expected="$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$archive" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  fi
  [ "$expected" = "$actual" ] || fail "checksum mismatch for downloaded cortex archive"

  # Extract the `cortex` binary from the release tarball (release.yml tars a
  # single `cortex` member at the archive root).
  case "$target" in
    macos-arm64) cp "$archive" "$tmpdir/cortex" ;;
    *) tar -xzf "$archive" -C "$tmpdir" cortex || fail "failed to extract cortex from archive" ;;
  esac
  [ -f "$tmpdir/cortex" ] || fail "release archive did not contain a cortex binary"
  chmod +x "$tmpdir/cortex"
  DOWNLOADED_PATH="$tmpdir/cortex"
  DOWNLOAD_TMPDIR="$tmpdir"
  DOWNLOAD_TMPDIR_CREATED="$CREATED_TMPDIR"
}

cleanup_download() {
  if [ -n "${INSTALL_STAGE:-}" ]; then rm -f "$INSTALL_STAGE"; fi
  if [ "${DOWNLOAD_TMPDIR_CREATED:-0}" = "1" ] && [ -n "${DOWNLOAD_TMPDIR:-}" ]; then
    rm -rf "$DOWNLOAD_TMPDIR"
  fi
}

configure_path() {
  case ":$PATH:" in *":$BIN_DIR:"*) return ;; esac
  PATH="$BIN_DIR:$PATH"; export PATH
  [ "${CORTEX_INSTALL_UPDATE_PATH:-1}" = "1" ] || return
  # Persist the same executable for future sessions without replacing any user
  # configuration. Shell-quote the directory so spaces and metacharacters work.
  quoted_bin="$(printf '%s' "$BIN_DIR" | sed "s/'/'\\\\''/g")"
  path_line="export PATH='$quoted_bin':\$PATH"
  case "${SHELL:-/bin/sh}" in
    */zsh) profiles="$HOME/.zshrc" ;;
    */bash)
      profiles="$HOME/.bashrc"
      if [ -f "$HOME/.bash_profile" ]; then login_profile="$HOME/.bash_profile";
      elif [ -f "$HOME/.bash_login" ]; then login_profile="$HOME/.bash_login";
      else login_profile="$HOME/.profile"; fi
      # HOME can contain spaces; handle each filename without word splitting.
      if ! grep -Fqx "$path_line" "$login_profile" 2>/dev/null; then
        printf '\n# Cortex CLI\n%s\n' "$path_line" >> "$login_profile"
      fi
      ;;
    */sh|*/dash|*/ksh) profiles="$HOME/.profile" ;;
    *) say "Add $BIN_DIR to PATH in your shell profile."; return ;;
  esac
  if ! grep -Fqx "$path_line" "$profiles" 2>/dev/null; then
    printf '\n# Cortex CLI\n%s\n' "$path_line" >> "$profiles"
    say "Added Cortex to $profiles; open a new shell to use cortex directly."
  fi
}

main() {
  if [ "$DRY_RUN" = "1" ]; then
    target="$(detect_target)"
    say "Dry run OK: target=$target prefix=$PREFIX repo=$REPO version=$VERSION method=$METHOD"
    exit 0
  fi

  trap cleanup_download EXIT
  trap 'exit 1' HUP INT TERM
  case "$METHOD" in
    pull)
      target="$(detect_target)"
      check_prereqs
      download_and_verify "$target"
      ;;
    build)
      build_from_source
      ;;
    *)
      fail "unknown METHOD '$METHOD'; expected pull or build"
      ;;
  esac

  mkdir -p "$BIN_DIR"
  INSTALL_STAGE="$(mktemp "$BIN_DIR/.cortex-install.XXXXXX")"
  install -m 0755 "$DOWNLOADED_PATH" "$INSTALL_STAGE"
  mv -f "$INSTALL_STAGE" "$BIN"
  INSTALL_STAGE=""
  say "Installed $BIN"

  configure_path

  if [ "$SKIP_SETUP" != "1" ]; then
    # Hand off to cortex setup. Prerequisite checks (data dir, port, env) run
    # inside `cortex setup start`.
    # curl | sh consumes stdin; preserve guided prompts through the controlling
    # terminal when invoked interactively. Automated runs retain their stdin.
    if [ -t 1 ] && [ -r /dev/tty ]; then
      "$BIN" setup start "$@" < /dev/tty
    else
      "$BIN" setup start "$@"
    fi
  fi
}

main "$@"
